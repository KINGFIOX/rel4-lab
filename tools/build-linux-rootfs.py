#!/usr/bin/env python3
"""Cross-compile the wave-1 Linux user programs and pack a newc rootfs cpio."""

from __future__ import annotations

import os
import sys
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parent))
from target_config import infer_toolprefix_for, target_from_env
from tool_common import (
    ELF_TYPE_EXECUTABLE,
    RISCV_ELF_MACHINE,
    ROOT_DIR,
    BuildLock,
    bare_metal_tool_env,
    default_linux_out_dir,
    die,
    getenv,
    linux_user_cflags,
    log,
    require_file,
    require_target_executable_elf,
    run,
)


PREFIX = "build-linux-rootfs"
ROOTFS_SRC = ROOT_DIR / "userspace" / "linux-rootfs"
LTP_UNAME01 = ROOT_DIR / "third_party" / "ltp" / "testcases" / "kernel" / "syscalls" / "uname" / "uname01.c"


def newc_hex(value: int) -> bytes:
    return f"{value:08X}".encode("ascii")


def add_newc(archive: bytearray, name: str, data: bytes, mode: int) -> None:
    name_bytes = name.encode("ascii") + b"\0"
    header = bytearray()
    header += b"070701"
    header += newc_hex(0)
    header += newc_hex(mode)
    header += newc_hex(0)
    header += newc_hex(0)
    header += newc_hex(1)
    header += newc_hex(0)
    header += newc_hex(len(data))
    header += newc_hex(0)
    header += newc_hex(0)
    header += newc_hex(0)
    header += newc_hex(0)
    header += newc_hex(len(name_bytes))
    header += newc_hex(0)
    archive.extend(header)
    archive.extend(name_bytes)
    while len(archive) % 4 != 0:
        archive.append(0)
    archive.extend(data)
    while len(archive) % 4 != 0:
        archive.append(0)


def write_cpio(out: Path, files: list[tuple[str, Path, int]], dirs: list[str]) -> None:
    archive = bytearray()
    for name in dirs:
        add_newc(archive, name, b"", 0o040755)
    for name, path, mode in files:
        add_newc(archive, name, path.read_bytes(), mode)
    add_newc(archive, "TRAILER!!!", b"", 0)
    out.parent.mkdir(parents=True, exist_ok=True)
    out.write_bytes(archive)


def compile_program(
    gcc: str,
    cflags: list[str],
    linker: Path,
    sources: list[str],
    output: Path,
    extra: list[str] | None = None,
) -> None:
    cmd = [
        gcc,
        *cflags,
        *sources,
        *(extra or []),
        "-Wl,-T",
        str(linker),
        "-Wl,--no-relax",
        "-Wl,-z,max-page-size=4096",
        "-o",
        str(output),
    ]
    run(cmd, env=bare_metal_tool_env())


def main() -> int:
    target = target_from_env(PREFIX)
    if target.name != "riscv64":
        die(PREFIX, f"linux-compat rootfs is RISC-V only; ARCH={target.name}")

    prefix = infer_toolprefix_for(target)
    if prefix is None:
        die(PREFIX, "no RISC-V gcc on PATH; activate the flake dev shell")
    gcc = f"{prefix}gcc"

    out_dir = Path(getenv("OUT_DIR", str(default_linux_out_dir(target))))
    bin_dir = out_dir / "bin"
    cpio_path = Path(getenv("LINUX_ROOTFS_CPIO", str(out_dir / "rootfs.cpio")))
    bin_dir.mkdir(parents=True, exist_ok=True)

    include = ROOTFS_SRC / "include"
    linker = ROOTFS_SRC / "linker.ld"
    require_file(PREFIX, linker, f"missing linker script: {linker}")
    require_file(PREFIX, LTP_UNAME01, f"missing LTP uname01: {LTP_UNAME01}")

    cflags = linux_user_cflags(include, target.linux_march, target.linux_mabi)
    crt = str(ROOTFS_SRC / "src" / "crt0.S")
    libc = str(ROOTFS_SRC / "src" / "libc.c")
    wave1_lib = str(ROOTFS_SRC / "src" / "wave1_lib.c")

    lock = BuildLock(ROOT_DIR)
    lock.acquire()
    try:
        log(PREFIX, f"compiling wave-1 programs with {gcc}")
        programs: list[tuple[str, Path]] = []

        uname_elf = bin_dir / "uname01"
        compile_program(
            gcc,
            cflags,
            linker,
            [crt, libc, str(ROOTFS_SRC / "src" / "wrap_ltp.c")],
            uname_elf,
            extra=[f"-DLTP_TEST_C=\"{LTP_UNAME01}\""],
        )
        programs.append(("uname01", uname_elf))

        for name in (
            "exit01",
            "write01",
            "open01",
            "getpid01",
            "fork01",
            "mkdir01",
            "chdir01",
            "getuid01",
            "clock_gettime01",
            "dup01",
            "pipe01",
        ):
            elf = bin_dir / name
            compile_program(
                gcc,
                cflags,
                linker,
                [crt, libc, wave1_lib, str(ROOTFS_SRC / "tests" / f"{name}.c")],
                elf,
            )
            programs.append((name, elf))

        runner = bin_dir / "ltp-wave1"
        compile_program(
            gcc,
            cflags,
            linker,
            [crt, libc, str(ROOTFS_SRC / "src" / "ltp-wave1.c")],
            runner,
        )
        programs.append(("ltp-wave1", runner))

        for name, elf in programs:
            require_target_executable_elf(PREFIX, target, elf, f"{name} ELF")
            data = elf.read_bytes()
            if int.from_bytes(data[16:18], "little") != ELF_TYPE_EXECUTABLE:
                die(PREFIX, f"{elf} is not ET_EXEC")
            if int.from_bytes(data[18:20], "little") != RISCV_ELF_MACHINE:
                die(PREFIX, f"{elf} is not RISC-V")

        files = [(name, path, 0o100755) for name, path in programs]
        write_cpio(cpio_path, files, ["tmp", "dev"])
        log(PREFIX, f"rootfs ready: {cpio_path}")
        print(cpio_path)
        return 0
    finally:
        lock.release()


if __name__ == "__main__":
    raise SystemExit(main())
