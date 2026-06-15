#!/usr/bin/env python3
"""Audit kernel/userspace seL4_UserContext register order assumptions."""

from __future__ import annotations

import argparse
import re
import sys
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parent))
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
    "loongarch64": list(range(32)),
}

EXPECTED_USERSPACE_REGS = {
    "riscv64": {
        "USER_CONTEXT_PC": 0,
        "USER_CONTEXT_RA": 1,
        "USER_CONTEXT_SP": 2,
        "USER_CONTEXT_A0": 10,
        "USER_CONTEXT_A1": 11,
    },
    "loongarch64": {
        "USER_CONTEXT_PC": 0,
        "USER_CONTEXT_RA": 1,
        "USER_CONTEXT_SP": 3,
        "USER_CONTEXT_A0": 4,
        "USER_CONTEXT_A1": 5,
    },
}

CONST_RE = re.compile(r"const\s+([A-Z0-9_]+)\s*:\s*usize\s*=\s*([0-9]+)\s*;")
ARRAY_RE = re.compile(
    r"pub\s+const\s+SEL4_USER_CONTEXT_REGS\s*:\s*\[[^\]]+\]\s*=\s*\[(?P<body>.*?)\];",
    re.S,
)
USER_REGISTER_RE = re.compile(r"UserRegister::([A-Za-z0-9_]+)\.index\(\)")

USER_REGISTER_INDEX = {
    "Ra": 1,
    "Tp": 2,
    "Sp": 3,
    "Gp": 3,
    "A0": 4,
    "A1": 5,
    "A2": 6,
    "A3": 7,
    "A4": 8,
    "A5": 9,
    "A6": 10,
    "A7": 11,
    "T0": 12,
    "T1": 13,
    "T2": 14,
    "T3": 15,
    "T4": 16,
    "T5": 17,
    "T6": 18,
    "T7": 19,
    "T8": 20,
    "R21": 21,
    "Fp": 22,
    "S0": 23,
    "S1": 24,
    "S2": 25,
    "S3": 26,
    "S4": 27,
    "S5": 28,
    "S6": 29,
    "S7": 30,
    "S8": 31,
}

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


def parse_kernel_context_regs(path: Path, target_name: str) -> list[int]:
    text = path.read_text()
    match = ARRAY_RE.search(text)
    if not match:
        die(PREFIX, f"SEL4_USER_CONTEXT_REGS array not found in {path}")
    body = match.group("body")
    regs: list[int] = []
    register_map = (
        USER_REGISTER_INDEX if target_name == "loongarch64" else RISCV_USER_REGISTER_INDEX
    )
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
    register_indexes = (
        USER_REGISTER_INDEX if target_name == "loongarch64" else RISCV_USER_REGISTER_INDEX
    )
    a0 = register_indexes["A0"]
    a1 = register_indexes["A1"]
    sp = register_indexes["Sp"]
    require_regex(
        errors,
        boot_rs,
        r"t\.context\.pc\s*=\s*args\.user_ventry\s+as\s+u64;.*?"
        r"t\.context\.restart_pc\s*=\s*args\.user_ventry\s+as\s+u64;.*?"
        r"t\.context\.sstatus\s*=\s*crate::arch::current::trap::ROOTSERVER_SSTATUS;",
        f"{target_name} rootserver PC/restart/sstatus initialisation",
    )
    require_regex(
        errors,
        boot_rs,
        rf"t\.context\.regs\[UserRegister::A0\.index\(\)\]\s*=\s*USER_BOOTINFO_VA\s+as\s+u64;.*?"
        rf"t\.context\.regs\[UserRegister::A1\.index\(\)\]\s*=\s*0;.*?"
        rf"t\.context\.regs\[UserRegister::Sp\.index\(\)\]\s*=\s*USER_STACK_TOP\s+as\s+u64;",
        f"{target_name} rootserver a0/a1/sp initialisation",
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
    trap_rs = ROOT_DIR / "kernel" / "src" / "arch" / target.name / "trap.rs"
    userspace_arch = ROOT_DIR / "userspace" / "xv6-host" / "src" / "arch" / f"{target.name}.rs"
    if not trap_rs.is_file():
        die(PREFIX, f"kernel trap source not found: {trap_rs}")
    if not userspace_arch.is_file():
        die(PREFIX, f"xv6-host arch source not found: {userspace_arch}")

    kernel_regs = parse_kernel_context_regs(trap_rs, target.name)
    expected_regs = EXPECTED_CONTEXT_REGS[target.name]
    errors: list[str] = []
    if kernel_regs != expected_regs:
        errors.append(f"kernel SEL4_USER_CONTEXT_REGS={kernel_regs}, expected {expected_regs}")
    audit_boot_rootserver_context(errors, target.name, kernel_regs)

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

    if errors:
        for error in errors:
            log(PREFIX, f"FAIL: {error}")
        return 1

    print(f"PASS: {target.name} seL4_UserContext ABI words={len(expected_regs)}")
    return 0


if __name__ == "__main__":
    raise SystemExit(main(sys.argv[1:]))
