#!/usr/bin/env python3
"""Audit architecture-critical Rust kernel ELF layout invariants."""

from __future__ import annotations

import argparse
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

ELFCLASS64 = 2
ELFDATA2LSB = 1
PT_LOAD = 1
PF_X = 1 << 0
PF_W = 1 << 1
PF_R = 1 << 2
PAGE_SIZE = 4096


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
class LayoutExpectation:
    machine: int
    entry: int
    vaddr_paddr_delta: int
    first_load_paddr: int
    max_load_end_paddr: int | None = None


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
    errors = [
        *validate_header(header, expectation),
        *validate_load_segments(program_headers, expectation),
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
