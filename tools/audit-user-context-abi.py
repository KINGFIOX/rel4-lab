#!/usr/bin/env python3
"""Audit kernel/userspace seL4_UserContext register order assumptions."""

from __future__ import annotations

import argparse
import re
import sys
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parent))
from kernel_arch_paths import sel4_arch_rs, trap_rs
from target_config import target_from_env
from tool_common import ROOT_DIR, die, log


PREFIX = "audit-user-context-abi"

EXPECTED_CONTEXT_REGS = {
    "riscv64": [
        0,
        1,
        2,
        3,
        8,
        9,
        18,
        19,
        20,
        21,
        22,
        23,
        24,
        25,
        26,
        27,
        10,
        11,
        12,
        13,
        14,
        15,
        16,
        17,
        5,
        6,
        7,
        28,
        29,
        30,
        31,
        4,
    ],
    "x86_64": [
        17,
        16,
        13,
        2,
        3,
        4,
        5,
        6,
        7,
        10,
        11,
        9,
        18,
        19,
        12,
        1,
        0,
        22,
        23,
        0,
        0,
        0,
        0,
        0,
        0,
        0,
        0,
        0,
        0,
        0,
        0,
        0,
    ],
}

EXPECTED_USERSPACE_REGS = {
    "riscv64": {
        "USER_CONTEXT_PC": 0,
        "USER_CONTEXT_RA": 1,
        "USER_CONTEXT_SP": 2,
        "USER_CONTEXT_A0": 10,
        "USER_CONTEXT_A1": 11,
    },
    "x86_64": {
        "USER_CONTEXT_PC": 0,
        "USER_CONTEXT_RA": 1,
        "USER_CONTEXT_SP": 7,
        "USER_CONTEXT_A0": 15,
        "USER_CONTEXT_A1": 14,
    },
}

CONST_RE = re.compile(r"const\s+([A-Z0-9_]+)\s*:\s*usize\s*=\s*([0-9]+)\s*;")
ARRAY_RE = re.compile(
    r"pub\s+const\s+SEL4_USER_CONTEXT_REGS\s*:\s*\[[^\]]+\]\s*=\s*\[(?P<body>.*?)\];",
    re.S,
)
USER_REGISTER_RE = re.compile(r"UserRegister::([A-Za-z0-9_]+)\.index\(\)")

RISCV_USER_REGISTER_INDEX = {
    "Ra": 1,
    "Sp": 2,
    "Gp": 3,
    "Tp": 4,
    "T0": 5,
    "A0": 10,
    "A1": 11,
    "A2": 12,
    "A3": 13,
    "A4": 14,
    "A5": 15,
    "A6": 16,
    "A7": 17,
}

X86_64_USER_REGISTER_INDEX = {
    "Rip": 17,
    "Rsp": 16,
    "Rflags": 13,
    "Rax": 2,
    "Rdi": 0,
    "Rsi": 1,
    "R10": 9,
    "R8": 10,
    "R9": 11,
    "R15": 12,
    "FsBase": 22,
    "A0": 0,
    "A1": 1,
    "Sp": 16,
}


def parse_kernel_context_regs(path: Path, target_name: str) -> list[int]:
    text = path.read_text()
    match = ARRAY_RE.search(text)
    if not match:
        die(PREFIX, f"SEL4_USER_CONTEXT_REGS array not found in {path}")
    body = match.group("body")
    regs: list[int] = []
    register_maps = {
        "riscv64": RISCV_USER_REGISTER_INDEX,
        "x86_64": X86_64_USER_REGISTER_INDEX,
    }
    register_map = register_maps[target_name]
    for item in body.split(","):
        item = item.strip()
        if not item:
            continue
        item = item.split("//", 1)[0].strip()
        if not item:
            continue
        if item.isdecimal():
            regs.append(int(item))
            continue
        reg_match = USER_REGISTER_RE.fullmatch(item)
        if reg_match:
            name = reg_match.group(1)
            if name not in register_map:
                die(PREFIX, f"unsupported UserRegister::{name} in {path}")
            regs.append(register_map[name])
            continue
        die(PREFIX, f"unsupported SEL4_USER_CONTEXT_REGS entry: {item}")
    return regs


def parse_userspace_consts(path: Path) -> dict[str, int]:
    return {name: int(value) for name, value in CONST_RE.findall(path.read_text())}


def expected_userspace_indexes(target_name: str, kernel_regs: list[int]) -> dict[str, int]:
    indexes = {"USER_CONTEXT_PC": 0}
    for name, reg in EXPECTED_USERSPACE_REGS[target_name].items():
        if name == "USER_CONTEXT_PC":
            continue
        if reg in kernel_regs:
            indexes[name] = kernel_regs.index(reg)
    return indexes


def require_regex(errors: list[str], path: Path, pattern: str, description: str) -> None:
    if re.search(pattern, path.read_text(), re.S) is None:
        errors.append(f"{path.relative_to(ROOT_DIR)} is missing {description}")


def audit_boot_rootserver_context(
    errors: list[str], target_name: str, kernel_regs: list[int]
) -> None:
    boot_rs = ROOT_DIR / "kernel" / "src" / "kernel" / "boot.rs"
    register_indexes = {
        "riscv64": RISCV_USER_REGISTER_INDEX,
        "x86_64": X86_64_USER_REGISTER_INDEX,
    }[target_name]
    a0 = register_indexes["A0"]
    a1 = register_indexes["A1"]
    sp = register_indexes["Sp"]
    require_regex(
        errors,
        boot_rs,
        r"sel4_arch::init_rootserver_context\(",
        f"{target_name} rootserver context initialisation",
    )
    expected = {
        "A0": a0,
        "A1": a1,
        "Sp": sp,
    }
    for name, reg in expected.items():
        if reg not in kernel_regs:
            errors.append(f"{target_name} rootserver {name} register index {reg} missing")


def main(argv: list[str]) -> int:
    parser = argparse.ArgumentParser(
        description="Check kernel and xv6-host seL4_UserContext ABI constants."
    )
    parser.parse_args(argv)

    target = target_from_env(PREFIX)
    kernel_regs_path = (
        sel4_arch_rs(target.name) if target.name == "x86_64" else trap_rs(target.name)
    )
    userspace_arch = ROOT_DIR / "userspace" / "xv6-host" / "src" / "arch" / f"{target.name}.rs"
    if not kernel_regs_path.is_file():
        die(PREFIX, f"kernel user-context source not found: {kernel_regs_path}")

    kernel_regs = parse_kernel_context_regs(kernel_regs_path, target.name)
    expected_regs = EXPECTED_CONTEXT_REGS[target.name]
    errors: list[str] = []
    if kernel_regs != expected_regs:
        errors.append(f"kernel SEL4_USER_CONTEXT_REGS={kernel_regs}, expected {expected_regs}")
    audit_boot_rootserver_context(errors, target.name, kernel_regs)

    if userspace_arch.is_file():
        userspace_consts = parse_userspace_consts(userspace_arch)
        words = userspace_consts.get("USER_CONTEXT_WORDS")
        if words != len(expected_regs):
            errors.append(f"USER_CONTEXT_WORDS={words}, expected {len(expected_regs)}")
        for name, expected in expected_userspace_indexes(target.name, kernel_regs).items():
            got = userspace_consts.get(name)
            if got is None and name == "USER_CONTEXT_RA" and target.name == "riscv64":
                continue
            if got != expected:
                errors.append(f"{name}={got}, expected {expected}")
    elif target.name != "x86_64":
        die(PREFIX, f"xv6-host arch source not found: {userspace_arch}")

    if errors:
        for error in errors:
            log(PREFIX, f"FAIL: {error}")
        return 1

    extra = "" if userspace_arch.is_file() else "; xv6-host not built for this target"
    print(f"PASS: {target.name} seL4_UserContext ABI words={len(expected_regs)}{extra}")
    return 0


if __name__ == "__main__":
    raise SystemExit(main(sys.argv[1:]))
