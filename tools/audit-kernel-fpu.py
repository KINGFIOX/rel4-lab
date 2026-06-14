#!/usr/bin/env python3
"""Audit that kernel FP-register/FCSR instructions stay in expected code."""

from __future__ import annotations

import argparse
import os
import re
import sys
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parent))
from tool_common import ROOT_DIR, command_exists, die, getenv, log, output, run


PREFIX = "audit-kernel-fpu"
DEFAULT_RUST_TARGET = "riscv64gc-unknown-none-elf"
DEFAULT_RISCV_ALLOWED_SOURCE = "kernel/src/arch/riscv64/fpu.rs"

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
    "movgr2fr.w",
    "movgr2fr.d",
    "movgr2fcsr",
    "movfr2gr.s",
    "movfr2gr.d",
    "movfcsr2gr",
}

LOONGARCH_FPU_PREFIXES = (
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
            return None
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
        LOONGARCH_FPU_PREFIXES
    )


def is_fpu_mnemonic(target: str, mnemonic: str, operands: str = "") -> bool:
    match target_arch(target):
        case "riscv64":
            return is_riscv_fpu_mnemonic(mnemonic, operands)
        case "loongarch64":
            return is_loongarch_fpu_mnemonic(mnemonic, operands)
        case _:
            raise AssertionError("unreachable target architecture")


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


def main(argv: list[str]) -> int:
    parser = argparse.ArgumentParser(
        description=(
            "Check that emitted kernel FP-register/FCSR instructions stay in "
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
            "defaults to the RISC-V FPU helper for RISC-V and no allowed "
            "source for LoongArch64"
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

    if args.build:
        log(PREFIX, f"building release kernel for {args.target}")
        run(["cargo", "build", "--release", "--target", args.target, "-p", "kernel"])

    if not elf.is_file():
        die(PREFIX, f"kernel ELF not found: {elf}")

    objdump = tool_name(args.target, "OBJDUMP", "objdump")
    addr2line = tool_name(args.target, "ADDR2LINE", "addr2line")
    objdump_output = output([objdump, "-d", str(elf)])
    addresses = fpu_instruction_addresses(args.target, objdump_output)
    locations = resolve_locations(addr2line, elf, addresses)
    offenders = [
        (address, location)
        for address, location in zip(addresses, locations, strict=True)
        if allowed_marker is None or allowed_marker not in location
    ]

    if offenders:
        allowed_description = allowed_source_arg or "no source file"
        print(
            f"FAIL: {len(offenders)} FP-register/FCSR instructions are outside {allowed_description}",
            file=sys.stderr,
        )
        for address, location in offenders:
            print(f"  {address}: {location}", file=sys.stderr)
        return 1

    if allowed_source_arg:
        print(
            f"PASS: {len(addresses)} FP-register/FCSR instructions confined to {allowed_source_arg}"
        )
    else:
        print(f"PASS: no FP-register/FCSR instructions found for {args.target}")
    if args.verbose:
        for location in sorted(set(locations)):
            print(f"  {location}")
    return 0


if __name__ == "__main__":
    raise SystemExit(main(sys.argv[1:]))
