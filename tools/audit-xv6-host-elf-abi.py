#!/usr/bin/env python3
"""Audit xv6-host ELF target checks used by payload and service loading."""

from __future__ import annotations

import re
import sys
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parent))
from target_config import target_from_env
from tool_common import (
    ELF_TYPE_EXECUTABLE,
    LOONGARCH64_ELF_MACHINE,
    RISCV_ELF_MACHINE,
    ROOT_DIR,
    log,
)


PREFIX = "audit-xv6-host-elf-abi"


def require_regex(errors: list[str], path: Path, pattern: str, description: str) -> None:
    if re.search(pattern, path.read_text(), re.S) is None:
        errors.append(f"{path.relative_to(ROOT_DIR)} is missing {description}")


def require_text(errors: list[str], path: Path, text: str, description: str) -> None:
    if text not in path.read_text():
        errors.append(f"{path.relative_to(ROOT_DIR)} is missing {description}: {text}")


def audit_build_script(errors: list[str]) -> None:
    path = ROOT_DIR / "userspace" / "xv6-host" / "build.rs"
    require_text(
        errors,
        path,
        f"const RISCV_ELF_MACHINE: u16 = {RISCV_ELF_MACHINE};",
        "RISC-V e_machine constant",
    )
    require_text(
        errors,
        path,
        f"const LOONGARCH64_ELF_MACHINE: u16 = {LOONGARCH64_ELF_MACHINE};",
        "LoongArch64 e_machine constant",
    )
    require_text(
        errors,
        path,
        f"const ELF_TYPE_EXECUTABLE: u16 = {ELF_TYPE_EXECUTABLE};",
        "ET_EXEC constant",
    )
    for name in (
        "payload",
        "uart_server",
        "vfs_server",
        "xv6fs_server",
        "disk_server",
    ):
        require_text(
            errors,
            path,
            f"validate_embedded_elf(&{name},",
            f"{name} embedded ELF validation",
        )
    require_regex(
        errors,
        path,
        r"fn\s+validate_embedded_elf\(.*?"
        r"data\.len\(\)\s*<\s*64.*?"
        r"data\[4\]\s*!=\s*2.*?"
        r"data\[5\]\s*!=\s*1.*?"
        r"elf_type\s*!=\s*ELF_TYPE_EXECUTABLE.*?"
        r"machine\s*!=\s*expected_machine",
        "embedded ELF class/data/type/machine validation",
    )


def audit_runtime_loader(errors: list[str]) -> None:
    path = ROOT_DIR / "userspace" / "xv6-host" / "src" / "child.rs"
    require_text(
        errors,
        path,
        f"const EXPECTED_ELF_MACHINE: u16 = {LOONGARCH64_ELF_MACHINE};",
        "LoongArch64 runtime e_machine constant",
    )
    require_text(
        errors,
        path,
        f"const EXPECTED_ELF_MACHINE: u16 = {RISCV_ELF_MACHINE};",
        "RISC-V runtime e_machine constant",
    )
    require_regex(
        errors,
        path,
        r"pub\(crate\)\s+fn\s+elf_image_valid\(elf:\s*&\[u8\]\)\s*->\s*bool\s*\{.*?"
        r"elf\.len\(\)\s*<\s*64.*?"
        r"u16::from_le_bytes\(\[elf\[16\],\s*elf\[17\]\]\)\s*!=\s*2.*?"
        r"u16::from_le_bytes\(\[elf\[18\],\s*elf\[19\]\]\)\s*!=\s*EXPECTED_ELF_MACHINE",
        "runtime ELF class/type/machine validation",
    )


def main() -> int:
    target = target_from_env(PREFIX)
    errors: list[str] = []
    audit_build_script(errors)
    audit_runtime_loader(errors)
    if errors:
        for error in errors:
            log(PREFIX, f"FAIL: {error}")
        return 1
    print(f"PASS: {target.name} xv6-host ELF ABI checks")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
