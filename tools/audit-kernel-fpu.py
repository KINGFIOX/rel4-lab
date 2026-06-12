#!/usr/bin/env python3
"""Audit that kernel FP-register/fcsr instructions stay in RISC-V FPU helpers."""

from __future__ import annotations

import argparse
import os
import re
import sys
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parent))
from tool_common import ROOT_DIR, die, getenv, infer_toolprefix, log, output, run


PREFIX = "audit-kernel-fpu"
DEFAULT_RUST_TARGET = "riscv64gc-unknown-none-elf"
DEFAULT_ALLOWED_SOURCE = "kernel/src/arch/riscv64/fpu.rs"

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


def tool_name(env_name: str, suffix: str) -> str:
    explicit = os.environ.get(env_name)
    if explicit:
        return explicit
    prefix = infer_toolprefix()
    if prefix is None:
        die(PREFIX, f"could not find a RISC-V toolchain for {suffix}")
    return f"{prefix}{suffix}"


def is_fpu_mnemonic(mnemonic: str, operands: str = "") -> bool:
    if mnemonic in FPU_PSEUDO_MNEMONICS or mnemonic.startswith(FPU_PREFIXES):
        return True
    if mnemonic not in CSR_MNEMONICS:
        return False
    return any(
        re.search(rf"\b{re.escape(register)}\b", operands)
        for register in CSR_FPU_REGISTERS
    )


def fpu_instruction_addresses(objdump_output: str) -> list[str]:
    addresses: list[str] = []
    for line in objdump_output.splitlines():
        match = INSTRUCTION_RE.match(line)
        if match is None:
            continue
        address, mnemonic, operands = match.groups()
        if is_fpu_mnemonic(mnemonic, operands or ""):
            addresses.append(f"0x{address}")
    return addresses


def resolve_locations(addr2line: str, elf: Path, addresses: list[str]) -> list[str]:
    if not addresses:
        return []
    locations = output([addr2line, "-e", str(elf), "-f", "-p", *addresses])
    return locations.splitlines()


def main(argv: list[str]) -> int:
    parser = argparse.ArgumentParser(
        description="Check that emitted kernel FP-register/fcsr instructions live in the RISC-V FPU helper file."
    )
    parser.add_argument(
        "elf",
        nargs="?",
        type=Path,
        help="kernel ELF to audit; defaults to the release kernel for RUST_TARGET",
    )
    parser.add_argument(
        "--allowed-source",
        default=DEFAULT_ALLOWED_SOURCE,
        help=f"repo-relative source file allowed to contain FPU instructions (default: {DEFAULT_ALLOWED_SOURCE})",
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
    allowed_source = (ROOT_DIR / args.allowed_source).resolve()
    allowed_marker = f"{allowed_source}:"

    if args.build:
        log(PREFIX, f"building release kernel for {args.target}")
        run(["cargo", "build", "--release", "--target", args.target, "-p", "kernel"])

    if not elf.is_file():
        die(PREFIX, f"kernel ELF not found: {elf}")

    objdump = tool_name("OBJDUMP", "objdump")
    addr2line = tool_name("ADDR2LINE", "addr2line")
    objdump_output = output([objdump, "-d", str(elf)])
    addresses = fpu_instruction_addresses(objdump_output)
    locations = resolve_locations(addr2line, elf, addresses)
    offenders = [
        (address, location)
        for address, location in zip(addresses, locations, strict=True)
        if allowed_marker not in location
    ]

    if offenders:
        print(
            f"FAIL: {len(offenders)} FP-register/fcsr instructions are outside {args.allowed_source}",
            file=sys.stderr,
        )
        for address, location in offenders:
            print(f"  {address}: {location}", file=sys.stderr)
        return 1

    print(
        f"PASS: {len(addresses)} FP-register/fcsr instructions confined to {args.allowed_source}"
    )
    if args.verbose:
        for location in sorted(set(locations)):
            print(f"  {location}")
    return 0


if __name__ == "__main__":
    raise SystemExit(main(sys.argv[1:]))
