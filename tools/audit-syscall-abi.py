#!/usr/bin/env python3
"""Audit kernel/userspace syscall ABI assumptions."""

from __future__ import annotations

import argparse
import re
import sys
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parent))
from kernel_arch_paths import trap_rs
from target_config import target_from_env
from tool_common import ROOT_DIR, log


PREFIX = "audit-syscall-abi"

KERNEL_SYSCALL_RE = re.compile(r"^\s*([A-Za-z][A-Za-z0-9_]*)\s*=\s*(-?\d+)\s*,", re.M)
USER_SYSCALL_RE = re.compile(
    r"pub\s+const\s+(SYS_[A-Z0-9_]+)\s*:\s*isize\s*=\s*(-?\d+)\s*;"
)
KERNEL_CONST_RE = re.compile(r"pub\s+const\s+([A-Z0-9_]+)\s*:\s*usize\s*=\s*(\d+)\s*;")

KERNEL_SYSCALLS = {
    "Call": -1,
    "ReplyRecv": -2,
    "Send": -3,
    "NonBlockingSend": -4,
    "Recv": -5,
    "Reply": -6,
    "Yield": -7,
    "NonBlockingRecv": -8,
    "DebugPutChar": -9,
    "DebugDumpScheduler": -10,
    "DebugHalt": -11,
    "DebugCapIdentify": -12,
    "DebugSnapshot": -13,
    "DebugNameThread": -14,
    "DebugSendIpi": -15,
}

KERNEL_OBJECT_SIZE_BITS = {
    "SEL4_SLOT_BITS": 5,
    "SEL4_TCB_BITS": 11,
    "SEL4_ENDPOINT_BITS": 4,
    "SEL4_NOTIFICATION_BITS": 5,
}

USER_TO_KERNEL_SYSCALLS = {
    "SYS_CALL": "Call",
    "SYS_REPLY_RECV": "ReplyRecv",
    "SYS_SEND": "Send",
    "SYS_NB_RECV": "NonBlockingRecv",
    "SYS_RECV": "Recv",
    "SYS_REPLY": "Reply",
    "SYS_WAIT": "Recv",
    "SYS_NB_WAIT": "NonBlockingRecv",
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
    "x86_64": {
        "instruction": '"syscall"',
        "register_prefix": "",
    },
}


def parse_kernel_syscalls(path: Path) -> dict[str, int]:
    return {name: int(value) for name, value in KERNEL_SYSCALL_RE.findall(path.read_text())}


def parse_userspace_syscalls(path: Path) -> dict[str, int]:
    return {name: int(value) for name, value in USER_SYSCALL_RE.findall(path.read_text())}


def parse_kernel_consts(path: Path) -> dict[str, int]:
    return {name: int(value) for name, value in KERNEL_CONST_RE.findall(path.read_text())}


def require_text(errors: list[str], path: Path, text: str, description: str) -> None:
    source = path.read_text()
    if text not in source:
        errors.append(f"{path.relative_to(ROOT_DIR)} is missing {description}: {text}")


def require_regex(errors: list[str], path: Path, pattern: str, description: str) -> None:
    source = path.read_text()
    if re.search(pattern, source, re.S) is None:
        errors.append(f"{path.relative_to(ROOT_DIR)} is missing {description}")


def arch_function_source(errors: list[str], path: Path, name: str) -> str:
    source = path.read_text()
    pattern = (
        rf"pub\(crate\)\s+unsafe\s+fn\s+{re.escape(name)}\s*"
        rf"\([^{{]*\)\s*(?:->\s*[^\{{]+)?\{{.*?\n\}}\n"
        rf"(?=\n#\[inline\(always\)\]|\Z)"
    )
    match = re.search(pattern, source, re.S)
    if match is None:
        errors.append(f"{path.relative_to(ROOT_DIR)} is missing {name} syscall stub")
        return ""
    return match.group(0)


def require_function_text(
    errors: list[str], path: Path, function: str, text: str, description: str
) -> None:
    if text not in function:
        errors.append(f"{path.relative_to(ROOT_DIR)} {description}: {text}")


def forbid_function_text(
    errors: list[str], path: Path, function: str, text: str, description: str
) -> None:
    if text in function:
        errors.append(f"{path.relative_to(ROOT_DIR)} unexpectedly has {description}: {text}")


def audit_syscall_numbers(errors: list[str]) -> None:
    kernel_path = ROOT_DIR / "kernel" / "src" / "abi" / "syscall.rs"
    kernel_syscalls = parse_kernel_syscalls(kernel_path)
    userspace_syscalls = parse_userspace_syscalls(
        ROOT_DIR / "userspace" / "sel4-user" / "src" / "lib.rs"
    )
    kernel_source = kernel_path.read_text()
    for kernel_name, expected_value in KERNEL_SYSCALLS.items():
        kernel_value = kernel_syscalls.get(kernel_name)
        if kernel_value is None:
            errors.append(f"kernel SyscallNumber::{kernel_name} is missing")
        elif kernel_value != expected_value:
            errors.append(
                f"SyscallNumber::{kernel_name}={kernel_value}, expected {expected_value}"
            )
        require_text(
            errors,
            kernel_path,
            f"{expected_value} => Some(Self::{kernel_name})",
            f"from_raw mapping for SyscallNumber::{kernel_name}",
        )
    extra_kernel_syscalls = set(kernel_syscalls) - set(KERNEL_SYSCALLS)
    for kernel_name in sorted(extra_kernel_syscalls):
        errors.append(f"kernel SyscallNumber::{kernel_name} is not in audited seL4 non-MCS set")
    if "non-MCS" not in kernel_source or "api-master" not in kernel_source:
        errors.append("kernel syscall source no longer documents the seL4 non-MCS api-master ABI")

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


def audit_object_size_bits(errors: list[str]) -> None:
    consts = parse_kernel_consts(ROOT_DIR / "kernel" / "src" / "abi" / "constants.rs")
    for name, expected_value in KERNEL_OBJECT_SIZE_BITS.items():
        value = consts.get(name)
        if value is None:
            errors.append(f"kernel {name} is missing")
        elif value != expected_value:
            errors.append(f"kernel {name}={value}, expected {expected_value}")


def audit_userspace_arch(errors: list[str], target_name: str) -> None:
    if target_name == "x86_64":
        return
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

    regs = {name: f'inlateout("{reg}{name}")' for name in ("a0", "a1", "a2", "a3", "a4", "a5", "a6", "a7")}
    call = arch_function_source(errors, path, "call")
    for name in ("a0", "a1", "a2", "a3", "a4", "a5"):
        require_function_text(errors, path, call, regs[name], f"call uses {name}")
    require_function_text(errors, path, call, regs["a7"], "call uses syscall-number register")
    require_function_text(errors, path, call, "SYS_CALL", "call uses SYS_CALL")
    forbid_function_text(errors, path, call, regs["a6"], "call reply-cap register")

    recv = arch_function_source(errors, path, "recv_with_reply")
    for name in ("a0", "a1", "a2", "a3", "a4", "a5", "a6", "a7"):
        require_function_text(errors, path, recv, regs[name], f"recv_with_reply uses {name}")
    require_function_text(errors, path, recv, "reply => _", "recv_with_reply passes reply cap")
    require_function_text(errors, path, recv, "syscall => _", "recv_with_reply passes syscall argument")

    wait = arch_function_source(errors, path, "wait")
    for name in ("a0", "a1", "a2", "a3", "a4", "a5", "a7"):
        require_function_text(errors, path, wait, regs[name], f"wait uses {name}")
    require_function_text(errors, path, wait, "syscall => _", "wait passes syscall argument")
    forbid_function_text(errors, path, wait, regs["a6"], "wait reply-cap register")

    reply_recv = arch_function_source(errors, path, "reply_recv_with_reply")
    for name in ("a0", "a1", "a2", "a3", "a4", "a5", "a6", "a7"):
        require_function_text(errors, path, reply_recv, regs[name], f"reply_recv_with_reply uses {name}")
    require_function_text(
        errors,
        path,
        reply_recv,
        "SYS_REPLY_RECV",
        "reply_recv_with_reply uses SYS_REPLY_RECV",
    )
    require_function_text(
        errors, path, reply_recv, "reply => _", "reply_recv_with_reply passes reply cap"
    )

    send = arch_function_source(errors, path, "send")
    for name in ("a0", "a1", "a2", "a3", "a4", "a5", "a7"):
        require_function_text(errors, path, send, regs[name], f"send uses {name}")
    require_function_text(errors, path, send, "SYS_SEND", "send uses SYS_SEND")
    forbid_function_text(errors, path, send, regs["a6"], "send reply-cap register")

    yield_now = arch_function_source(errors, path, "yield_now")
    require_function_text(errors, path, yield_now, regs["a7"], "yield_now uses a7")
    require_function_text(errors, path, yield_now, "SYS_YIELD", "yield_now uses SYS_YIELD")

    debug_put_char = arch_function_source(errors, path, "debug_put_char")
    require_function_text(errors, path, debug_put_char, regs["a0"], "debug_put_char uses a0")
    require_function_text(errors, path, debug_put_char, regs["a7"], "debug_put_char uses a7")
    require_function_text(
        errors,
        path,
        debug_put_char,
        "SYS_DEBUG_PUT_CHAR",
        "debug_put_char uses SYS_DEBUG_PUT_CHAR",
    )

    debug_halt = arch_function_source(errors, path, "debug_halt")
    require_function_text(errors, path, debug_halt, regs["a7"], "debug_halt uses a7")
    require_function_text(
        errors, path, debug_halt, "SYS_DEBUG_HALT", "debug_halt uses SYS_DEBUG_HALT"
    )


def audit_userspace_common(errors: list[str]) -> None:
    path = ROOT_DIR / "userspace" / "sel4-user" / "src" / "lib.rs"
    for syscall in USER_TO_KERNEL_SYSCALLS:
        require_text(errors, path, syscall, f"{syscall} definition/use")


def audit_kernel_trap(errors: list[str], target_name: str) -> None:
    if target_name == "x86_64":
        return
    path = trap_rs(target_name)
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

    for kernel_name in KERNEL_SYSCALLS:
        if f"SyscallNumber::{kernel_name}" not in source:
            errors.append(f"{rel} does not dispatch SyscallNumber::{kernel_name}")
    require_regex(
        errors,
        path,
        r"Some\(SyscallNumber::DebugPutChar\)\s*=>\s*\{\s*"
        r"let\s+ch\s*=\s*uc\.regs\[UserRegister::A0\.index\(\)\]\s+as\s+u8;\s*"
        r"crate::machine::console::putc\(ch\);",
        "DebugPutChar syscall handler",
    )
    require_regex(
        errors,
        path,
        r"Some\(SyscallNumber::DebugCapIdentify\)\s*=>\s*\{.*?"
        r"let\s+cptr\s*=\s*uc\.regs\[UserRegister::A0\.index\(\)\];.*?"
        r"crate::api::cspace::lookup_cap\(thread,\s*cptr\).*?"
        r"uc\.regs\[UserRegister::A0\.index\(\)\]\s*=\s*tag;",
        "DebugCapIdentify syscall handler",
    )
    require_regex(
        errors,
        path,
        r"Some\(SyscallNumber::Yield\)\s*=>\s*unsafe\s*\{\s*"
        r"let\s+cur\s*=\s*crate::object::tcb::current\(\);\s*"
        r"if\s+!cur\.is_null\(\)\s*\{\s*"
        r"crate::object::tcb::rotate_to_tail\(cur\);\s*\}\s*\}",
        "Yield syscall handler",
    )
    require_regex(
        errors,
        path,
        r"Some\(SyscallNumber::Call\)\s*=>\s*\{\s*crate::api::syscall::do_call\(uc\);\s*\}",
        "Call syscall handler",
    )
    require_regex(
        errors,
        path,
        r"Some\(SyscallNumber::Send\)\s*=>\s*\{\s*crate::api::syscall::do_send\(uc,\s*false\);\s*\}",
        "Send syscall handler",
    )
    require_regex(
        errors,
        path,
        r"Some\(SyscallNumber::NonBlockingSend\)\s*=>\s*\{\s*crate::api::syscall::do_send\(uc,\s*true\);\s*\}",
        "NonBlockingSend syscall handler",
    )
    require_regex(
        errors,
        path,
        r"Some\(SyscallNumber::Recv\s*\|\s*SyscallNumber::NonBlockingRecv\)\s*=>\s*\{\s*"
        r"let\s+blocking\s*=\s*SyscallNumber::from_raw\(raw_sysno\)\s*==\s*Some\(SyscallNumber::Recv\);\s*"
        r"crate::api::syscall::do_recv\(uc,\s*blocking\);\s*\}",
        "Recv and NonBlockingRecv syscall handler",
    )
    require_regex(
        errors,
        path,
        r"Some\(SyscallNumber::Reply\)\s*=>\s*\{\s*crate::api::ipc::reply\(uc\);\s*\}",
        "Reply syscall handler",
    )
    require_regex(
        errors,
        path,
        r"Some\(SyscallNumber::ReplyRecv\)\s*=>\s*\{\s*crate::api::ipc::reply_recv\(uc\);\s*\}",
        "ReplyRecv syscall handler",
    )
    require_regex(
        errors,
        path,
        r"None\s*=>\s*\{\s*if\s*!send_unknown_syscall_fault\(uc,\s*raw_sysno\)\s*\{\s*"
        r"warn!\(.*?unknown\s+(?:loongarch64\s+)?syscall number.*?\);\s*"
        r"park_current_thread\(\);\s*\}\s*\}",
        "unknown syscall fallback",
    )
    require_regex(
        errors,
        path,
        r"uc\.pc\s*=\s*uc\.pc\.wrapping_add\(4\);",
        "4-byte syscall PC advance",
    )


def main(argv: list[str]) -> int:
    parser = argparse.ArgumentParser(
        description="Check kernel and sel4-user syscall ABI constants/registers."
    )
    parser.parse_args(argv)

    target = target_from_env(PREFIX)
    errors: list[str] = []
    audit_syscall_numbers(errors)
    audit_object_size_bits(errors)
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
