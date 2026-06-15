#!/usr/bin/env python3
"""Audit kernel/userspace syscall ABI assumptions."""

from __future__ import annotations

import argparse
import re
import sys
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parent))
from target_config import target_from_env
from tool_common import ROOT_DIR, log


PREFIX = "audit-syscall-abi"

KERNEL_SYSCALL_RE = re.compile(r"^\s*([A-Za-z][A-Za-z0-9_]*)\s*=\s*(-?\d+)\s*,", re.M)
USER_SYSCALL_RE = re.compile(
    r"pub\s+const\s+(SYS_[A-Z0-9_]+)\s*:\s*isize\s*=\s*(-?\d+)\s*;"
)

USER_TO_KERNEL_SYSCALLS = {
    "SYS_CALL": "Call",
    "SYS_REPLY_RECV": "ReplyRecv",
    "SYS_SEND": "Send",
    "SYS_NB_RECV": "NonBlockingRecv",
    "SYS_RECV": "Recv",
    "SYS_WAIT": "Wait",
    "SYS_NB_WAIT": "NonBlockingWait",
    "SYS_YIELD": "Yield",
    "SYS_DEBUG_PUT_CHAR": "DebugPutChar",
    "SYS_DEBUG_HALT": "DebugHalt",
}

ARCH_EXPECTATIONS = {
    "riscv64": {
        "instruction": '"ecall"',
        "register_prefix": "",
    },
    "loongarch64": {
        "instruction": '"syscall 0"',
        "register_prefix": "$",
    },
}


def parse_kernel_syscalls(path: Path) -> dict[str, int]:
    return {name: int(value) for name, value in KERNEL_SYSCALL_RE.findall(path.read_text())}


def parse_userspace_syscalls(path: Path) -> dict[str, int]:
    return {name: int(value) for name, value in USER_SYSCALL_RE.findall(path.read_text())}


def require_text(errors: list[str], path: Path, text: str, description: str) -> None:
    source = path.read_text()
    if text not in source:
        errors.append(f"{path.relative_to(ROOT_DIR)} is missing {description}: {text}")


def require_regex(errors: list[str], path: Path, pattern: str, description: str) -> None:
    source = path.read_text()
    if re.search(pattern, source, re.S) is None:
        errors.append(f"{path.relative_to(ROOT_DIR)} is missing {description}")


def audit_syscall_numbers(errors: list[str]) -> None:
    kernel_syscalls = parse_kernel_syscalls(ROOT_DIR / "kernel" / "src" / "abi" / "syscall.rs")
    userspace_syscalls = parse_userspace_syscalls(
        ROOT_DIR / "userspace" / "sel4-user" / "src" / "lib.rs"
    )
    for user_name, kernel_name in USER_TO_KERNEL_SYSCALLS.items():
        kernel_value = kernel_syscalls.get(kernel_name)
        user_value = userspace_syscalls.get(user_name)
        if kernel_value is None:
            errors.append(f"kernel SyscallNumber::{kernel_name} is missing")
        if user_value is None:
            errors.append(f"userspace {user_name} is missing")
        if kernel_value is not None and user_value is not None and kernel_value != user_value:
            errors.append(
                f"{user_name}={user_value}, expected SyscallNumber::{kernel_name}={kernel_value}"
            )


def audit_userspace_arch(errors: list[str], target_name: str) -> None:
    expectation = ARCH_EXPECTATIONS[target_name]
    path = ROOT_DIR / "userspace" / "sel4-user" / "src" / "arch" / f"{target_name}.rs"
    text = path.read_text()
    instruction = expectation["instruction"]
    reg = expectation["register_prefix"]

    if text.count(instruction) < 7:
        errors.append(f"{path.relative_to(ROOT_DIR)} has too few syscall instructions")
    require_text(errors, path, f'inlateout("{reg}a0")', "a0 syscall argument/result register")
    require_text(errors, path, f'inlateout("{reg}a1")', "a1 syscall argument/result register")
    require_text(errors, path, f'inlateout("{reg}a2")', "a2 message register")
    require_text(errors, path, f'inlateout("{reg}a3")', "a3 message register")
    require_text(errors, path, f'inlateout("{reg}a4")', "a4 message register")
    require_text(errors, path, f'inlateout("{reg}a5")', "a5 message register")
    require_text(errors, path, f'inlateout("{reg}a6")', "a6 reply register")
    require_text(errors, path, f'inlateout("{reg}a7")', "a7 syscall-number register")
    require_text(errors, path, 'clobber_abi("C")', "C ABI clobber declaration")
    require_text(errors, path, "options(nostack)", "nostack asm option")
    for syscall in (
        "SYS_CALL",
        "SYS_REPLY_RECV",
        "SYS_SEND",
        "SYS_YIELD",
        "SYS_DEBUG_PUT_CHAR",
        "SYS_DEBUG_HALT",
    ):
        require_text(errors, path, syscall, f"{syscall} use")


def audit_userspace_common(errors: list[str]) -> None:
    path = ROOT_DIR / "userspace" / "sel4-user" / "src" / "lib.rs"
    for syscall in USER_TO_KERNEL_SYSCALLS:
        require_text(errors, path, syscall, f"{syscall} definition/use")


def audit_kernel_trap(errors: list[str], target_name: str) -> None:
    path = ROOT_DIR / "kernel" / "src" / "arch" / target_name / "trap.rs"
    rel = path.relative_to(ROOT_DIR)
    source = path.read_text()

    require_text(
        errors,
        path,
        "uc.regs[UserRegister::A7.index()] as isize",
        "syscall number read from A7",
    )
    require_text(errors, path, "uc.pc = uc.pc.wrapping_add(4);", "4-byte syscall PC advance")
    require_text(errors, path, "uc.regs[UserRegister::A0.index()]", "A0 syscall argument/result use")

    for kernel_name in set(USER_TO_KERNEL_SYSCALLS.values()):
        if f"SyscallNumber::{kernel_name}" not in source:
            errors.append(f"{rel} does not dispatch SyscallNumber::{kernel_name}")
    require_regex(
        errors,
        path,
        r"SyscallNumber::from_raw\(raw_sysno\).*None\s*=>",
        "unknown-syscall fallback",
    )


def main(argv: list[str]) -> int:
    parser = argparse.ArgumentParser(
        description="Check kernel and sel4-user syscall ABI constants/registers."
    )
    parser.parse_args(argv)

    target = target_from_env(PREFIX)
    errors: list[str] = []
    audit_syscall_numbers(errors)
    audit_userspace_common(errors)
    audit_userspace_arch(errors, target.name)
    audit_kernel_trap(errors, target.name)

    if errors:
        for error in errors:
            log(PREFIX, f"FAIL: {error}")
        return 1

    print(f"PASS: {target.name} syscall ABI constants and registers")
    return 0


if __name__ == "__main__":
    raise SystemExit(main(sys.argv[1:]))
