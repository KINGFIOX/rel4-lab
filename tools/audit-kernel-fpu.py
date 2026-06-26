#!/usr/bin/env python3
"""Audit that kernel FP/SIMD state instructions stay in expected code."""

from __future__ import annotations

import argparse
import os
import re
import sys
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parent))
from tool_common import (
    LOONGARCH64_EFLAGS_ABI_MASK,
    LOONGARCH64_EFLAGS_ABI_SOFT_FLOAT,
    ROOT_DIR,
    command_exists,
    die,
    getenv,
    log,
    output,
    run,
)


PREFIX = "audit-kernel-fpu"
DEFAULT_RUST_TARGET = "riscv64gc-unknown-none-elf"
DEFAULT_RISCV_ALLOWED_SOURCE = "kernel/src/arch/riscv64/fpu.rs"
DEFAULT_LOONGARCH_ALLOWED_SOURCE = "kernel/src/arch/loongarch64/fpu.rs"

INSTRUCTION_RE = re.compile(
    r"^\s*([0-9a-fA-F]+):(?:\s+[0-9a-fA-F]{2,8})+\s+([A-Za-z0-9_.]+)\b(?:\s+(.*))?$"
)

FPU_PSEUDO_MNEMONICS = {
    "fld",
    "flw",
    "flh",
    "flq",
    "fsd",
    "fsw",
    "fsh",
    "fsq",
    "frcsr",
    "fscsr",
    "frrm",
    "fsrm",
    "fsrmi",
    "frflags",
    "fsflags",
    "fsflagsi",
}

FPU_PREFIXES = (
    "fmadd.",
    "fmsub.",
    "fnmadd.",
    "fnmsub.",
    "fadd.",
    "fsub.",
    "fmul.",
    "fdiv.",
    "fsqrt.",
    "fsgnj.",
    "fsgnjn.",
    "fsgnjx.",
    "fmin.",
    "fmax.",
    "fcvt.",
    "fmv.",
    "feq.",
    "flt.",
    "fle.",
    "fclass.",
)

LOONGARCH_FPU_PSEUDO_MNEMONICS = {
    "fld.s",
    "fld.d",
    "fst.s",
    "fst.d",
    "movcf2gr",
    "movgr2cf",
    "movgr2fr.w",
    "movgr2fr.d",
    "movgr2fcsr",
    "movfr2gr.s",
    "movfr2gr.d",
    "movfcsr2gr",
}

LOONGARCH_SCALAR_FPU_PREFIXES = (
    "fadd.",
    "fsub.",
    "fmul.",
    "fdiv.",
    "fmax.",
    "fmin.",
    "fmaxa.",
    "fmina.",
    "fabs.",
    "fneg.",
    "flogb.",
    "fclass.",
    "fsqrt.",
    "frecip.",
    "frsqrt.",
    "fmov.",
    "fcopysign.",
    "fscaleb.",
    "fcvt.",
    "ftint.",
    "ffint.",
    "frint.",
    "fcmp.",
)

LOONGARCH_VECTOR_PREFIXES = (
    "v",
    "xv",
)

CSR_FPU_REGISTERS = {
    "fcsr",
    "fflags",
    "frm",
}

CSR_MNEMONICS = {
    "csrr",
    "csrw",
    "csrs",
    "csrc",
    "csrwi",
    "csrsi",
    "csrci",
    "csrrw",
    "csrrs",
    "csrrc",
    "csrrwi",
    "csrrsi",
    "csrrci",
}


def target_arch(target: str) -> str:
    if target.startswith("riscv64"):
        return "riscv64"
    if target.startswith("loongarch64"):
        return "loongarch64"
    die(PREFIX, f"unsupported Rust target for FPU audit: {target}")


def default_allowed_source(target: str) -> str | None:
    match target_arch(target):
        case "riscv64":
            return DEFAULT_RISCV_ALLOWED_SOURCE
        case "loongarch64":
            return DEFAULT_LOONGARCH_ALLOWED_SOURCE
        case _:
            raise AssertionError("unreachable target architecture")


def tool_name(target: str, env_name: str, suffix: str) -> str:
    explicit = os.environ.get(env_name)
    if explicit:
        return explicit
    prefix = infer_toolprefix(target)
    if prefix is None:
        die(PREFIX, f"could not find a {target_arch(target)} toolchain for {suffix}")
    return f"{prefix}{suffix}"


def infer_toolprefix(target: str) -> str | None:
    prefixes = {
        "riscv64": (
            "riscv64-none-elf-",
            "riscv64-unknown-elf-",
            "riscv64-elf-",
            "riscv64-linux-gnu-",
            "riscv64-unknown-linux-gnu-",
        ),
        "loongarch64": (
            "loongarch64-none-elf-",
            "loongarch64-unknown-none-",
            "loongarch64-unknown-linux-gnu-",
            "loongarch64-linux-gnu-",
        ),
    }[target_arch(target)]
    for prefix in prefixes:
        if command_exists(f"{prefix}gcc"):
            return prefix
    return None


def is_riscv_fpu_mnemonic(mnemonic: str, operands: str = "") -> bool:
    if mnemonic in FPU_PSEUDO_MNEMONICS or mnemonic.startswith(FPU_PREFIXES):
        return True
    if mnemonic not in CSR_MNEMONICS:
        return False
    return any(
        re.search(rf"\b{re.escape(register)}\b", operands)
        for register in CSR_FPU_REGISTERS
    )


def is_loongarch_fpu_mnemonic(mnemonic: str, _operands: str = "") -> bool:
    return mnemonic in LOONGARCH_FPU_PSEUDO_MNEMONICS or mnemonic.startswith(
        LOONGARCH_SCALAR_FPU_PREFIXES + LOONGARCH_VECTOR_PREFIXES
    )


def is_loongarch_vector_mnemonic(mnemonic: str) -> bool:
    return mnemonic.startswith(LOONGARCH_VECTOR_PREFIXES)


def is_fpu_mnemonic(target: str, mnemonic: str, operands: str = "") -> bool:
    match target_arch(target):
        case "riscv64":
            return is_riscv_fpu_mnemonic(mnemonic, operands)
        case "loongarch64":
            return is_loongarch_fpu_mnemonic(mnemonic, operands)
        case _:
            raise AssertionError("unreachable target architecture")


def require_source_regex(errors: list[str], path: Path, pattern: str, description: str) -> None:
    if re.search(pattern, path.read_text(), re.S) is None:
        errors.append(f"{path.relative_to(ROOT_DIR)} is missing {description}")


def validate_loongarch_fpu_source() -> None:
    fpu_rs = ROOT_DIR / "kernel" / "src" / "arch" / "loongarch64" / "fpu.rs"
    trap_rs = ROOT_DIR / "kernel" / "src" / "arch" / "loongarch64" / "trap.rs"
    boot_rs = ROOT_DIR / "kernel" / "src" / "arch" / "loongarch64" / "boot.rs"
    errors: list[str] = []

    require_source_regex(
        errors,
        fpu_rs,
        r"const\s+EUEN_FPE\s*:\s*usize\s*=\s*1\s*<<\s*0\s*;",
        "LoongArch FPU enable bit",
    )
    require_source_regex(
        errors,
        fpu_rs,
        r"const\s+EUEN_SXE\s*:\s*usize\s*=\s*1\s*<<\s*1\s*;",
        "LoongArch LSX enable bit",
    )
    require_source_regex(
        errors,
        fpu_rs,
        r"const\s+EUEN_ASXE\s*:\s*usize\s*=\s*1\s*<<\s*2\s*;",
        "LoongArch LASX enable bit",
    )
    require_source_regex(
        errors,
        fpu_rs,
        r"const\s+EUEN_FPU_STATE_MASK\s*:\s*usize\s*="
        r"\s*EUEN_FPE\s*\|\s*EUEN_SXE\s*\|\s*EUEN_ASXE\s*;",
        "combined FPU/LSX/LASX state mask",
    )
    require_source_regex(
        errors,
        fpu_rs,
        r"pub\(crate\)\s+const\s+EUEN_FPU_STATE_CLEAR_MASK\s*:\s*i64\s*="
        r"\s*!\(EUEN_FPU_STATE_MASK\s+as\s+i64\)\s*;",
        "derived FPU/LSX/LASX clear mask",
    )
    require_source_regex(
        errors,
        fpu_rs,
        r"fn\s+clear_fpu_enable\(\)\s*\{.*?"
        r"set_euen\(euen\s*&\s*!EUEN_FPU_STATE_MASK\);.*?"
        r"csr::dbar\(\);.*?\}",
        "EUEN FPU/LSX/LASX clear helper with barrier",
    )
    require_source_regex(
        errors,
        fpu_rs,
        r"fn\s+set_scalar_fpu_enable\(\)\s*\{.*?"
        r"set_euen\(\(euen\s*\|\s*EUEN_FPE\)\s*&\s*!EUEN_VECTOR_STATE_MASK\);.*?"
        r"csr::dbar\(\);.*?\}",
        "scalar FPU enable helper keeps LSX/LASX disabled",
    )
    require_source_regex(
        errors,
        fpu_rs,
        r"unsafe\s+fn\s+save_fpu_state\(thread:\s*\*mut Tcb\).*?"
        r"fst\.d\s+\$f0.*?"
        r"fst\.d\s+\$f31.*?"
        r"dest\.fcsr\s*=\s*read_fcsr\(\);.*?"
        r"dest\.fcc\s*=\s*read_fcc\(\);",
        "LoongArch scalar FPU save covers f0-f31, fcsr, and fcc",
    )
    require_source_regex(
        errors,
        fpu_rs,
        r"fn\s+read_fcc\(\)\s*->\s*u64.*?"
        r"movcf2gr\s+\{fcc0\},\s+\$fcc0.*?"
        r"movcf2gr\s+\{fcc7\},\s+\$fcc7",
        "LoongArch scalar FPU save reads fcc0-fcc7",
    )
    require_source_regex(
        errors,
        fpu_rs,
        r"unsafe\s+fn\s+load_fpu_state\(thread:\s*\*const Tcb\).*?"
        r"fld\.d\s+\$f0.*?"
        r"fld\.d\s+\$f31.*?"
        r"write_fcsr\(src\.fcsr\);.*?"
        r"write_fcc\(src\.fcc\);",
        "LoongArch scalar FPU load covers f0-f31, fcsr, and fcc",
    )
    require_source_regex(
        errors,
        fpu_rs,
        r"fn\s+write_fcc\(fcc:\s*u64\).*?"
        r"movgr2cf\s+\$fcc0,\s+\{fcc0\}.*?"
        r"movgr2cf\s+\$fcc7,\s+\{fcc7\}",
        "LoongArch scalar FPU load writes fcc0-fcc7",
    )
    require_source_regex(
        errors,
        fpu_rs,
        r"pub\s+fn\s+lazy_restore\(thread:\s*\*mut Tcb\)\s*\{.*?"
        r"fpu_disabled_snapshot\(thread\).*?"
        r"disable_access\(\);.*?"
        r"tcb::set_fpu_context_enabled\(thread,\s*false\).*?"
        r"switch_local_owner\(thread\)",
        "lazy restore honors fpuDisabled and switches scalar FPU owner",
    )
    require_source_regex(
        errors,
        trap_rs,
        r"pub\s+struct\s+FpuState\s*\{.*?"
        r"pub\s+regs:\s*\[u64;\s*LOONGARCH_NUM_FP_REGS\].*?"
        r"pub\s+fcsr:\s*u32.*?"
        r"pub\s+fcc:\s*u64.*?\}",
        "LoongArch scalar FPU state in UserContext",
    )
    require_source_regex(
        errors,
        boot_rs,
        r'"csrrd\s+\$t0,\s+\{csr_euen\}".*?'
        r'"li\.d\s+\$t1,\s+\{euen_fpu_state_clear_mask\}".*?'
        r'"and\s+\$t0,\s+\$t0,\s+\$t1".*?'
        r'"csrwr\s+\$t0,\s+\{csr_euen\}".*?'
        r'"dbar\s+0".*?'
        r"csr_euen\s*=\s*const\s+crate::arch::loongarch64::csr::CSR_EUEN.*?"
        r"euen_fpu_state_clear_mask\s*=\s*const\s+"
        r"crate::arch::loongarch64::fpu::EUEN_FPU_STATE_CLEAR_MASK",
        "early EUEN FPU/LSX/LASX clear barrier before Rust entry using CSR and mask constants",
    )

    if errors:
        for error in errors:
            log(PREFIX, f"FAIL: {error}")
        raise SystemExit(1)


def fpu_instruction_addresses(target: str, objdump_output: str) -> list[str]:
    addresses: list[str] = []
    for line in objdump_output.splitlines():
        match = INSTRUCTION_RE.match(line)
        if match is None:
            continue
        address, mnemonic, operands = match.groups()
        if is_fpu_mnemonic(target, mnemonic, operands or ""):
            addresses.append(f"0x{address}")
    return addresses


def resolve_locations(addr2line: str, elf: Path, addresses: list[str]) -> list[str]:
    if not addresses:
        return []
    locations = output([addr2line, "-e", str(elf), "-f", "-p", *addresses])
    return locations.splitlines()


def loongarch_abi_name(elf: Path) -> str:
    data = elf.read_bytes()
    if len(data) < 52:
        die(PREFIX, f"ELF header too small: {elf}")
    flags = int.from_bytes(data[48:52], "little")
    abi = flags & LOONGARCH64_EFLAGS_ABI_MASK
    if abi == LOONGARCH64_EFLAGS_ABI_SOFT_FLOAT:
        return "soft-float"
    if abi == 0x2:
        return "single-float"
    if abi == 0x3:
        return "double-float"
    return f"unknown({abi:#x})"


def main(argv: list[str]) -> int:
    parser = argparse.ArgumentParser(
        description=(
            "Check that emitted kernel FP/SIMD state instructions stay in "
            "architecture-approved code."
        )
    )
    parser.add_argument(
        "elf",
        nargs="?",
        type=Path,
        help="kernel ELF to audit; defaults to the release kernel for RUST_TARGET",
    )
    parser.add_argument(
        "--allowed-source",
        default=None,
        help=(
            "repo-relative source file allowed to contain FPU instructions; "
            "defaults to the architecture FPU helper for RISC-V and LoongArch64"
        ),
    )
    parser.add_argument(
        "--build",
        action="store_true",
        help="build the release kernel before auditing",
    )
    parser.add_argument(
        "--target",
        default=getenv("RUST_TARGET", DEFAULT_RUST_TARGET),
        help=f"Rust target used with --build and the default ELF path (default: {DEFAULT_RUST_TARGET})",
    )
    parser.add_argument(
        "--verbose",
        action="store_true",
        help="print the unique allowed source locations that contain FPU instructions",
    )
    args = parser.parse_args(argv)

    elf = args.elf or ROOT_DIR / "target" / args.target / "release" / "kernel"
    elf = elf.expanduser().resolve()
    allowed_source_arg = (
        args.allowed_source
        if args.allowed_source is not None
        else default_allowed_source(args.target)
    )
    allowed_marker = None
    if allowed_source_arg:
        allowed_source = (ROOT_DIR / allowed_source_arg).resolve()
        allowed_marker = f"{allowed_source}:"

    arch = target_arch(args.target)
    if arch == "loongarch64":
        validate_loongarch_fpu_source()

    if args.build:
        log(PREFIX, f"building release kernel for {args.target}")
        run(["cargo", "build", "--release", "--target", args.target, "-p", "kernel"])

    if not elf.is_file():
        die(PREFIX, f"kernel ELF not found: {elf}")

    objdump = tool_name(args.target, "OBJDUMP", "objdump")
    addr2line = tool_name(args.target, "ADDR2LINE", "addr2line")
    objdump_output = output([objdump, "-d", str(elf)])
    addresses = fpu_instruction_addresses(args.target, objdump_output)
    if arch == "loongarch64":
        vector_addresses = []
        for line in objdump_output.splitlines():
            match = INSTRUCTION_RE.match(line)
            if match is None:
                continue
            address, mnemonic, _operands = match.groups()
            if is_loongarch_vector_mnemonic(mnemonic):
                vector_addresses.append(f"0x{address}")
        if vector_addresses:
            locations = resolve_locations(addr2line, elf, vector_addresses)
            print(
                f"FAIL: {len(vector_addresses)} LoongArch LSX/LASX instructions found",
                file=sys.stderr,
            )
            for address, location in zip(vector_addresses, locations, strict=True):
                print(f"  {address}: {location}", file=sys.stderr)
            return 1

    locations = resolve_locations(addr2line, elf, addresses)
    offenders = [
        (address, location)
        for address, location in zip(addresses, locations, strict=True)
        if allowed_marker is None or allowed_marker not in location
    ]

    if offenders:
        allowed_description = allowed_source_arg or "no source file"
        print(
            f"FAIL: {len(offenders)} FP/SIMD state instructions are outside {allowed_description}",
            file=sys.stderr,
        )
        for address, location in offenders:
            print(f"  {address}: {location}", file=sys.stderr)
        return 1

    abi_suffix = (
        f" (ELF ABI: {loongarch_abi_name(elf)})" if arch == "loongarch64" else ""
    )
    if allowed_source_arg:
        print(
            f"PASS: {len(addresses)} FP/SIMD state instructions confined to {allowed_source_arg}{abi_suffix}"
        )
    else:
        print(f"PASS: no FP/SIMD state instructions found for {args.target}{abi_suffix}")
    if args.verbose:
        for location in sorted(set(locations)):
            print(f"  {location}")
    return 0


if __name__ == "__main__":
    raise SystemExit(main(sys.argv[1:]))
