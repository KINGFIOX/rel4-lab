#!/usr/bin/env python3
"""Audit architecture-critical Rust kernel ELF layout invariants."""

from __future__ import annotations

import argparse
import re
import struct
import sys
from dataclasses import dataclass
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parent))
from target_config import rust_target_from_env, target_from_env
from tool_common import (
    ELF_TYPE_EXECUTABLE,
    LOONGARCH64_ELF_MACHINE,
    RISCV_ELF_MACHINE,
    ROOT_DIR,
    die,
    log,
    run,
)


PREFIX = "audit-kernel-elf"

ELF_HEADER = struct.Struct("<16sHHIQQQIHHHHHH")
PROGRAM_HEADER = struct.Struct("<IIQQQQQQ")
SECTION_HEADER = struct.Struct("<IIQQQQIIQQ")
SYMBOL = struct.Struct("<IBBHQQ")

ELFCLASS64 = 2
ELFDATA2LSB = 1
PT_LOAD = 1
SHT_SYMTAB = 2
PF_X = 1 << 0
PF_W = 1 << 1
PF_R = 1 << 2
PAGE_SIZE = 4096
RUST_USIZE_CONST_RE = re.compile(
    r"pub\s+const\s+(?P<name>[A-Z0-9_]+)\s*:\s*usize\s*=\s*(?P<expr>[^;]+);"
)
BOOT_STACK_IMMEDIATE_RE = re.compile(
    r'"li(?:\.d)?\s+(?:\$)?t1,\s*(?P<value>[0-9_]+)"'
)
BOOT_STACK_CONST_RE = re.compile(
    r"kernel_stack_bytes\s*=\s*const\s+crate::kernel::smp::KERNEL_STACK_BYTES"
)
BOOT_FN_RE = re.compile(
    r"pub\s+extern\s+\"C\"\s+fn\s+(?P<name>init_kernel|init_secondary_hart)\s*"
    r"\((?P<body>.*?)\)\s*->\s*!",
    re.S,
)
BOOT_ARGS_RE = re.compile(r"pub\s+struct\s+BootArgs\s*\{(?P<body>.*?)\}", re.S)
BOOT_ARGS_INIT_RE = re.compile(
    r"crate::kernel::boot::BootArgs\s*\{(?P<body>.*?)\}",
    re.S,
)
RUST_FIELD_RE = re.compile(r"pub\s+([A-Za-z_][A-Za-z0-9_]*)\s*:")
RUST_PARAM_RE = re.compile(r"(_?[A-Za-z][A-Za-z0-9_]*)\s*:\s*usize")
RUST_INIT_FIELD_RE = re.compile(r"\b([A-Za-z_][A-Za-z0-9_]*)\s*,")

EXPECTED_BOOT_HANDOFF_FIELDS = (
    "user_pstart",
    "user_pend",
    "pv_offset",
    "user_ventry",
    "dtb_pa",
    "dtb_size",
    "hart_id",
    "core_id",
)


@dataclass(frozen=True)
class ElfHeader:
    ident: bytes
    elf_type: int
    machine: int
    version: int
    entry: int
    phoff: int
    shoff: int
    flags: int
    ehsize: int
    phentsize: int
    phnum: int
    shentsize: int
    shnum: int
    shstrndx: int


@dataclass(frozen=True)
class ProgramHeader:
    p_type: int
    flags: int
    offset: int
    vaddr: int
    paddr: int
    filesz: int
    memsz: int
    align: int

    @property
    def is_load(self) -> bool:
        return self.p_type == PT_LOAD

    @property
    def end_vaddr(self) -> int:
        return self.vaddr + self.memsz

    @property
    def end_paddr(self) -> int:
        return self.paddr + self.memsz


@dataclass(frozen=True)
class SectionHeader:
    name_offset: int
    sh_type: int
    flags: int
    addr: int
    offset: int
    size: int
    link: int
    info: int
    addralign: int
    entsize: int


@dataclass(frozen=True)
class Symbol:
    name: str
    value: int
    size: int
    info: int
    other: int
    shndx: int


@dataclass(frozen=True)
class LayoutExpectation:
    machine: int
    entry: int
    vaddr_paddr_delta: int
    first_load_paddr: int
    max_load_end_paddr: int | None = None


@dataclass(frozen=True)
class StackExpectation:
    per_hart_bytes: int
    max_harts: int

    @property
    def total_bytes(self) -> int:
        return self.per_hart_bytes * self.max_harts


EXPECTATIONS = {
    "riscv64": LayoutExpectation(
        machine=RISCV_ELF_MACHINE,
        entry=0xFFFF_FFFF_8020_0000,
        vaddr_paddr_delta=0xFFFF_FFFF_0000_0000,
        first_load_paddr=0x8020_0000,
    ),
    "loongarch64": LayoutExpectation(
        machine=LOONGARCH64_ELF_MACHINE,
        entry=0x0020_0000,
        vaddr_paddr_delta=0,
        first_load_paddr=0x0020_0000,
        # The in-tree LoongArch boot path reserves low memory below 32 MiB for
        # kernel/rootserver/loader staging before high RAM untypeds begin.
        max_load_end_paddr=0x0200_0000,
    ),
}


def target_arch(target_name: str) -> str:
    if target_name == "riscv64":
        return "riscv64"
    if target_name == "loongarch64":
        return "loongarch64"
    die(PREFIX, f"unsupported ARCH={target_name}")


def eval_rust_usize_expr(expr: str, constants: dict[str, int]) -> int:
    text = expr.split("//", 1)[0].strip()
    for name, value in sorted(constants.items(), key=lambda item: len(item[0]), reverse=True):
        text = re.sub(rf"\b{re.escape(name)}\b", str(value), text)
    text = text.replace("_", "")
    if not re.fullmatch(r"[0-9()+*\- /]+", text):
        die(PREFIX, f"unsupported Rust usize expression: {expr}")
    return int(eval(text, {"__builtins__": {}}, {}))


def read_smp_stack_expectation() -> StackExpectation:
    path = ROOT_DIR / "kernel" / "src" / "kernel" / "smp.rs"
    constants: dict[str, int] = {}
    for match in RUST_USIZE_CONST_RE.finditer(path.read_text()):
        name = match.group("name")
        if name not in ("MAX_BOOT_HARTS", "KERNEL_STACK_BYTES"):
            continue
        constants[name] = eval_rust_usize_expr(match.group("expr"), constants)
    missing = {"MAX_BOOT_HARTS", "KERNEL_STACK_BYTES"} - constants.keys()
    if missing:
        die(PREFIX, f"missing SMP stack constants in {path}: {', '.join(sorted(missing))}")
    return StackExpectation(
        per_hart_bytes=constants["KERNEL_STACK_BYTES"],
        max_harts=constants["MAX_BOOT_HARTS"],
    )


def validate_boot_stack_source(arch: str) -> list[str]:
    path = ROOT_DIR / "kernel" / "src" / "arch" / arch / "boot.rs"
    text = path.read_text()
    errors: list[str] = []
    if BOOT_STACK_IMMEDIATE_RE.search(text):
        errors.append(f"{path.relative_to(ROOT_DIR)} still hard-codes the boot stack stride")
    if not BOOT_STACK_CONST_RE.search(text):
        errors.append(
            f"{path.relative_to(ROOT_DIR)} does not bind boot stack stride to KERNEL_STACK_BYTES"
        )
    return errors


def source_field_list(body: str, pattern: re.Pattern[str]) -> list[str]:
    return [match.group(1).lstrip("_") for match in pattern.finditer(body)]


def validate_boot_handoff_source(arch: str) -> list[str]:
    shared_boot = ROOT_DIR / "kernel" / "src" / "kernel" / "boot.rs"
    arch_boot = ROOT_DIR / "kernel" / "src" / "arch" / arch / "boot.rs"
    shared_text = shared_boot.read_text()
    arch_text = arch_boot.read_text()
    expected = list(EXPECTED_BOOT_HANDOFF_FIELDS)
    errors: list[str] = []

    boot_args_match = BOOT_ARGS_RE.search(shared_text)
    if boot_args_match is None:
        errors.append(f"{shared_boot.relative_to(ROOT_DIR)} is missing BootArgs")
    else:
        fields = source_field_list(boot_args_match.group("body"), RUST_FIELD_RE)
        if fields != expected:
            errors.append(f"BootArgs fields are {fields}, expected {expected}")

    functions = {
        match.group("name"): match.group("body")
        for match in BOOT_FN_RE.finditer(arch_text)
    }
    for name in ("init_kernel", "init_secondary_hart"):
        body = functions.get(name)
        if body is None:
            errors.append(f"{arch_boot.relative_to(ROOT_DIR)} is missing {name}")
            continue
        params = source_field_list(body, RUST_PARAM_RE)
        if params != expected:
            errors.append(
                f"{arch_boot.relative_to(ROOT_DIR)} {name} params are {params}, expected {expected}"
            )

    init_matches = BOOT_ARGS_INIT_RE.findall(arch_text)
    if not init_matches:
        errors.append(f"{arch_boot.relative_to(ROOT_DIR)} does not construct BootArgs")
    else:
        fields = source_field_list(init_matches[0], RUST_INIT_FIELD_RE)
        if fields != expected:
            errors.append(
                f"{arch_boot.relative_to(ROOT_DIR)} BootArgs initializer fields are "
                f"{fields}, expected {expected}"
            )

    for name in ("init_kernel", "init_secondary_hart"):
        if f"{name} = sym {name}" not in arch_text:
            errors.append(
                f"{arch_boot.relative_to(ROOT_DIR)} does not bind {name} as an asm symbol"
            )
        if f"{{{name}}}" not in arch_text:
            errors.append(
                f"{arch_boot.relative_to(ROOT_DIR)} does not call {name} from _start asm"
            )

    return errors


def require_source_regex(
    errors: list[str], path: Path, text: str, pattern: str, description: str
) -> None:
    if re.search(pattern, text, re.S) is None:
        errors.append(f"{path.relative_to(ROOT_DIR)} is missing {description}")


def validate_arch_boot_source(arch: str) -> list[str]:
    path = ROOT_DIR / "kernel" / "src" / "arch" / arch / "boot.rs"
    text = path.read_text()
    errors: list[str] = []

    if arch == "loongarch64":
        require_source_regex(
            errors,
            path,
            text,
            r'"la\.local\s+\$t0,\s+__stack_top".*?'
            r'"li\.d\s+\$t1,\s+\{kernel_stack_bytes\}".*?'
            r'"mul\.d\s+\$t1,\s+\$a7,\s+\$t1".*?'
            r'"sub\.d\s+\$sp,\s+\$t0,\s+\$t1".*?'
            r"kernel_stack_bytes\s*=\s*const\s+crate::kernel::smp::KERNEL_STACK_BYTES",
            "LoongArch per-core boot stack selected from handoff core_id",
        )
        require_source_regex(
            errors,
            path,
            text,
            r'"bnez\s+\$a7,\s+4f".*?'
            r'"la\.local\s+\$t0,\s+__bss_start".*?'
            r'"la\.local\s+\$t1,\s+__bss_end".*?'
            r'"st\.d\s+\$zero,\s+\$t0,\s+0".*?'
            r'"addi\.d\s+\$t0,\s+\$t0,\s+8".*?'
            r'"la\.local\s+\$t0,\s+__stack_top".*?'
            r'"move\s+\$sp,\s+\$t0".*?'
            r'"bl\s+\{init_kernel\}"',
            "LoongArch boot hart clears BSS before init_kernel handoff",
        )
        require_source_regex(
            errors,
            path,
            text,
            r'"csrwr\s+\$zero,\s+\{csr_ks0\}".*?'
            r"csr_ks0\s*=\s*const\s+crate::arch::loongarch64::csr::CSR_KS0",
            "LoongArch KS0 scratch clear before Rust entry using CSR constant",
        )
        require_source_regex(
            errors,
            path,
            text,
            r'"csrrd\s+\$t0,\s+\{csr_euen\}".*?'
            r'"li\.d\s+\$t1,\s+\{euen_fpu_state_clear_mask\}".*?'
            r'"and\s+\$t0,\s+\$t0,\s+\$t1".*?'
            r'"csrwr\s+\$t0,\s+\{csr_euen\}".*?'
            r'"dbar\s+0".*?'
            r"csr_euen\s*=\s*const\s+crate::arch::loongarch64::csr::CSR_EUEN.*?"
            r"euen_fpu_state_clear_mask\s*=\s*const\s+"
            r"crate::arch::loongarch64::fpu::EUEN_FPU_STATE_CLEAR_MASK",
            "LoongArch early FPU/LSX/LASX disable barrier before Rust entry using CSR and mask constants",
        )
        require_source_regex(
            errors,
            path,
            text,
            r'"ld\.d\s+\$t1,\s+\$t0,\s+0".*?'
            r'"bne\s+\$t1,\s+\$t2,\s+5b".*?'
            r'"dbar\s+0".*?'
            r'"bl\s+\{init_secondary_hart\}"',
            "LoongArch secondary ready acquire barrier before Rust entry",
        )
        require_source_regex(
            errors,
            path,
            text,
            r"pub\s+extern\s+\"C\"\s+fn\s+init_secondary_hart\([^)]*\)\s*->\s*!\s*\{"
            r".*?smp::init_current_hart\(hart_id,\s*core_id\);"
            r".*?fpu::init_current_core\(\);"
            r".*?vspace::switch_satp\(satp\)"
            r".*?trap::install_trap_vector\(\);"
            r".*?trap::init_timer\(\);"
            r".*?irq::init_current_core\(\);"
            r".*?trap::idle_scheduler_loop\(\)",
            "LoongArch secondary hart arch initialisation sequence",
        )

    return errors


def parse_header(data: bytes) -> ElfHeader:
    if len(data) < ELF_HEADER.size:
        die(PREFIX, "file is too small to be an ELF64 executable")
    return ElfHeader(*ELF_HEADER.unpack_from(data, 0))


def parse_program_headers(data: bytes, header: ElfHeader) -> list[ProgramHeader]:
    if header.phentsize != PROGRAM_HEADER.size:
        die(PREFIX, f"unexpected program-header entry size: {header.phentsize}")
    end = header.phoff + header.phnum * header.phentsize
    if header.phoff > len(data) or end > len(data):
        die(PREFIX, "program-header table extends past end of file")
    headers = []
    for index in range(header.phnum):
        offset = header.phoff + index * header.phentsize
        headers.append(ProgramHeader(*PROGRAM_HEADER.unpack_from(data, offset)))
    return headers


def parse_section_headers(data: bytes, header: ElfHeader) -> list[SectionHeader]:
    if header.shnum == 0:
        return []
    if header.shentsize != SECTION_HEADER.size:
        die(PREFIX, f"unexpected section-header entry size: {header.shentsize}")
    end = header.shoff + header.shnum * header.shentsize
    if header.shoff > len(data) or end > len(data):
        die(PREFIX, "section-header table extends past end of file")
    sections = []
    for index in range(header.shnum):
        offset = header.shoff + index * header.shentsize
        sections.append(SectionHeader(*SECTION_HEADER.unpack_from(data, offset)))
    return sections


def c_string(data: bytes, offset: int) -> str:
    if offset >= len(data):
        return ""
    end = data.find(b"\0", offset)
    if end == -1:
        end = len(data)
    return data[offset:end].decode(errors="replace")


def section_bytes(data: bytes, section: SectionHeader) -> bytes:
    end = section.offset + section.size
    if section.offset > len(data) or end > len(data):
        die(PREFIX, "section extends past end of file")
    return data[section.offset:end]


def parse_symbols(data: bytes, sections: list[SectionHeader]) -> dict[str, Symbol]:
    symbols: dict[str, Symbol] = {}
    for section in sections:
        if section.sh_type != SHT_SYMTAB:
            continue
        if section.entsize != SYMBOL.size:
            die(PREFIX, f"unexpected symbol entry size: {section.entsize}")
        if section.link >= len(sections):
            die(PREFIX, "symbol table references a missing string table")
        strings = section_bytes(data, sections[section.link])
        table = section_bytes(data, section)
        for offset in range(0, len(table), SYMBOL.size):
            name_offset, info, other, shndx, value, size = SYMBOL.unpack_from(table, offset)
            name = c_string(strings, name_offset)
            if name:
                symbols[name] = Symbol(
                    name=name,
                    value=value,
                    size=size,
                    info=info,
                    other=other,
                    shndx=shndx,
                )
    return symbols


def is_power_of_two(value: int) -> bool:
    return value != 0 and value & (value - 1) == 0


def validate_header(header: ElfHeader, expectation: LayoutExpectation) -> list[str]:
    errors: list[str] = []
    ident = header.ident
    if ident[:4] != b"\x7fELF":
        errors.append("missing ELF magic")
    if ident[4] != ELFCLASS64:
        errors.append(f"expected ELF64 class, found {ident[4]}")
    if ident[5] != ELFDATA2LSB:
        errors.append(f"expected little-endian ELF, found data encoding {ident[5]}")
    if header.elf_type != ELF_TYPE_EXECUTABLE:
        errors.append(f"expected ET_EXEC, found e_type={header.elf_type:#x}")
    if header.machine != expectation.machine:
        errors.append(
            f"unexpected e_machine={header.machine:#x}, expected {expectation.machine:#x}"
        )
    if header.entry != expectation.entry:
        errors.append(f"unexpected entry={header.entry:#x}, expected {expectation.entry:#x}")
    if header.ehsize != ELF_HEADER.size:
        errors.append(f"unexpected ELF header size: {header.ehsize}")
    return errors


def validate_load_segments(
    program_headers: list[ProgramHeader], expectation: LayoutExpectation
) -> list[str]:
    errors: list[str] = []
    loads = [ph for ph in program_headers if ph.is_load]
    if not loads:
        return ["no PT_LOAD segments found"]

    first = loads[0]
    if first.paddr != expectation.first_load_paddr:
        errors.append(
            f"first PT_LOAD paddr={first.paddr:#x}, expected {expectation.first_load_paddr:#x}"
        )
    if first.vaddr != expectation.entry:
        errors.append(
            f"first PT_LOAD vaddr={first.vaddr:#x}, expected entry {expectation.entry:#x}"
        )
    if (first.flags & (PF_R | PF_W | PF_X)) != (PF_R | PF_W | PF_X):
        errors.append(f"first PT_LOAD must be boot RWX, found flags={first.flags:#x}")

    executable_entry_segment = False
    max_load_end_paddr = 0
    for idx, segment in enumerate(loads):
        if segment.memsz < segment.filesz:
            errors.append(f"PT_LOAD[{idx}] memsz is smaller than filesz")
        if segment.align < PAGE_SIZE or not is_power_of_two(segment.align):
            errors.append(f"PT_LOAD[{idx}] has invalid alignment {segment.align:#x}")
        if segment.offset % segment.align != segment.vaddr % segment.align:
            errors.append(f"PT_LOAD[{idx}] file offset is not congruent with vaddr")
        if segment.vaddr - segment.paddr != expectation.vaddr_paddr_delta:
            errors.append(
                f"PT_LOAD[{idx}] vaddr-paddr delta is "
                f"{segment.vaddr - segment.paddr:#x}, expected {expectation.vaddr_paddr_delta:#x}"
            )
        if (
            segment.flags & PF_X
            and segment.vaddr <= expectation.entry
            and expectation.entry < segment.end_vaddr
        ):
            executable_entry_segment = True
        max_load_end_paddr = max(max_load_end_paddr, segment.end_paddr)

    if not executable_entry_segment:
        errors.append("entry point is not covered by an executable PT_LOAD segment")
    if (
        expectation.max_load_end_paddr is not None
        and max_load_end_paddr > expectation.max_load_end_paddr
    ):
        errors.append(
            f"PT_LOAD memory ends at {max_load_end_paddr:#x}, above staging limit "
            f"{expectation.max_load_end_paddr:#x}"
        )
    return errors


def validate_symbols(
    symbols: dict[str, Symbol],
    program_headers: list[ProgramHeader],
    expectation: LayoutExpectation,
    stack_expectation: StackExpectation,
) -> list[str]:
    errors: list[str] = []
    required = (
        "_start",
        "__kernel_start",
        "__boot_bss_start",
        "__boot_bss_end",
        "__boot_end",
        "__bss_start",
        "__stack_bottom",
        "__stack_top",
        "__bss_end",
        "__kernel_end",
    )
    for name in required:
        if name not in symbols:
            errors.append(f"missing linker symbol {name}")
    if errors:
        return errors

    def sym(name: str) -> int:
        return symbols[name].value

    if sym("_start") != expectation.entry:
        errors.append(f"_start={sym('_start'):#x}, expected {expectation.entry:#x}")
    if sym("__kernel_start") != expectation.entry:
        errors.append(
            f"__kernel_start={sym('__kernel_start'):#x}, expected {expectation.entry:#x}"
        )
    if sym("__boot_bss_start") > sym("__boot_bss_end"):
        errors.append("__boot_bss_start is after __boot_bss_end")
    if sym("__boot_bss_end") > sym("__boot_end"):
        errors.append("__boot_bss_end is after __boot_end")
    if sym("__boot_end") % PAGE_SIZE != 0:
        errors.append(f"__boot_end is not page-aligned: {sym('__boot_end'):#x}")
    if sym("__bss_start") % PAGE_SIZE != 0:
        errors.append(f"__bss_start is not page-aligned: {sym('__bss_start'):#x}")
    if not (
        sym("__bss_start")
        <= sym("__stack_bottom")
        < sym("__stack_top")
        <= sym("__bss_end")
        <= sym("__kernel_end")
    ):
        errors.append("BSS/stack/kernel-end linker symbols are not ordered")
    stack_bytes = sym("__stack_top") - sym("__stack_bottom")
    if stack_bytes != stack_expectation.total_bytes:
        errors.append(
            f"kernel stack reserve is {stack_bytes:#x}, "
            f"expected {stack_expectation.total_bytes:#x} "
            f"({stack_expectation.max_harts} * {stack_expectation.per_hart_bytes:#x})"
        )
    if sym("__bss_end") % 8 != 0:
        errors.append(f"__bss_end is not 8-byte aligned: {sym('__bss_end'):#x}")
    if sym("__kernel_end") % PAGE_SIZE != 0:
        errors.append(f"__kernel_end is not page-aligned: {sym('__kernel_end'):#x}")

    loads = [ph for ph in program_headers if ph.is_load]
    for name in required:
        value = sym(name)
        if name == "__kernel_end":
            continue
        if not any(segment.vaddr <= value <= segment.end_vaddr for segment in loads):
            errors.append(f"{name}={value:#x} is not covered by a PT_LOAD memory range")
    return errors


def main(argv: list[str]) -> int:
    parser = argparse.ArgumentParser(
        description="Check Rust kernel ELF layout invariants used by boot and packing."
    )
    parser.add_argument(
        "elf",
        nargs="?",
        type=Path,
        help="kernel ELF to audit; defaults to the release kernel for ARCH/RUST_TARGET",
    )
    parser.add_argument(
        "--build",
        action="store_true",
        help="build the release kernel before auditing",
    )
    args = parser.parse_args(argv)

    target = target_from_env(PREFIX)
    arch = target_arch(target.name)
    expectation = EXPECTATIONS[arch]
    stack_expectation = read_smp_stack_expectation()
    rust_target = rust_target_from_env(target)

    if args.build:
        log(PREFIX, f"building release kernel for {rust_target}")
        run(["cargo", "build", "--release", "--target", rust_target, "-p", "kernel"])

    elf = args.elf or ROOT_DIR / "target" / rust_target / "release" / "kernel"
    elf = elf.expanduser().resolve()
    if not elf.is_file():
        die(PREFIX, f"kernel ELF not found: {elf}")

    data = elf.read_bytes()
    header = parse_header(data)
    program_headers = parse_program_headers(data, header)
    sections = parse_section_headers(data, header)
    symbols = parse_symbols(data, sections)
    errors = [
        *validate_header(header, expectation),
        *validate_load_segments(program_headers, expectation),
        *validate_symbols(symbols, program_headers, expectation, stack_expectation),
        *validate_boot_stack_source(arch),
        *validate_boot_handoff_source(arch),
        *validate_arch_boot_source(arch),
    ]
    if errors:
        for error in errors:
            log(PREFIX, f"FAIL: {error}")
        return 1

    print(
        f"PASS: {arch} kernel ELF entry={expectation.entry:#x} "
        f"loads={sum(1 for ph in program_headers if ph.is_load)}"
    )
    return 0


if __name__ == "__main__":
    raise SystemExit(main(sys.argv[1:]))
