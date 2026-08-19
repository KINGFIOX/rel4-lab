#!/usr/bin/env python3
"""Build linux-compat, pack a RISC-V image, and run the wave-1 LTP gate."""

from __future__ import annotations

import os
import subprocess
import sys
import time
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parent))
from target_config import (
    image_suffix_from_env,
    rust_target_from_env,
    sel4_build_dir_from_env,
    sel4_tree_dir_from_env,
    target_from_env,
)
from tool_common import (
    ROOT_DIR,
    BuildLock,
    LoggedProcess,
    default_linux_out_dir,
    die,
    ensure_rust_log_at_least_info,
    file_has_regex,
    getenv,
    log,
    qemu_smp_arg,
    require_file,
    require_target_executable_elf,
    run,
    tail_lines,
)


PREFIX = "run-ltp"
SUCCESS_RE = r"ltp-wave1: ok"
FAIL_RE = r"KERNEL PANIC|linux-compat: panic|linux-compat: fault kill|ltp-wave1: fail"


def check_output_last_line(cmd: list[str], env: dict[str, str] | None = None) -> str:
    output = subprocess.check_output(cmd, env=env, text=True)
    lines = [line.strip() for line in output.splitlines() if line.strip()]
    if not lines:
        die(PREFIX, f"no output from {' '.join(cmd)}")
    return lines[-1]


def rust_env() -> dict[str, str]:
    env = os.environ.copy()
    env.pop("CARGO_TARGET_DIR", None)
    return env


def main() -> int:
    ensure_rust_log_at_least_info()
    target = target_from_env(PREFIX)
    if target.name != "riscv64":
        die(PREFIX, f"run-ltp.py is the RISC-V linux-compat gate; ARCH={target.name} is unsupported")

    rust_target = rust_target_from_env(target)
    out_dir = default_linux_out_dir(target)
    timeout = int(getenv("TIMEOUT", "180"))
    smp = qemu_smp_arg("1")
    verbose = "--verbose" in sys.argv or "-v" in sys.argv
    run_id = getenv("LINUX_RUN_ID", f"ltp-{os.getpid()}")
    packed_image = Path(getenv("OUT_IMAGE", str(ROOT_DIR / "images" / f"linux-{run_id}-{image_suffix_from_env(target)}")))
    log_file = Path(getenv("LOG_FILE", str(ROOT_DIR / "target" / f"linux-{run_id}-last-run.log")))
    kernel_debug_log_file = Path(
        getenv("KERNEL_DEBUG_LOG_FILE", str(ROOT_DIR / "target" / f"linux-{run_id}-kernel-debug.log"))
    )
    host_elf = ROOT_DIR / "target" / rust_target / "release" / "linux-compat"
    uart_elf = ROOT_DIR / "target" / rust_target / "release" / "uart-server"
    vfs_elf = ROOT_DIR / "target" / rust_target / "release" / "vfs-server"

    target.require_qemu(PREFIX)
    target.require_sel4_arch_source(PREFIX, sel4_tree_dir_from_env(sel4_build_dir_from_env(target)))

    lock = BuildLock(ROOT_DIR)
    lock.acquire()
    try:
        log(PREFIX, "building linux rootfs")
        cpio = Path(
            check_output_last_line([str(ROOT_DIR / "tools" / "build-linux-rootfs.py")], env=os.environ.copy())
        )
        require_file(PREFIX, cpio, f"rootfs cpio missing: {cpio}")

        cargo_env = rust_env()
        log(PREFIX, f"building uart-server and vfs-server target={rust_target}")
        run(
            [
                "cargo",
                "build",
                "--release",
                "--target",
                rust_target,
                "-p",
                "uart-server",
                "-p",
                "vfs-server",
            ],
            cwd=ROOT_DIR,
            env={**cargo_env, "LINUX_ROOTFS_CPIO": str(cpio)},
        )
        require_target_executable_elf(PREFIX, target, uart_elf, "uart-server ELF")
        require_target_executable_elf(PREFIX, target, vfs_elf, "vfs-server ELF")

        log(PREFIX, "auditing linux-compat ELF ABI checks")
        run([sys.executable, "tools/audit-linux-compat-elf-abi.py"], cwd=ROOT_DIR, env=os.environ.copy())

        log(PREFIX, "building linux-compat")
        run(
            [
                "cargo",
                "build",
                "--release",
                "--target",
                rust_target,
                "-p",
                "linux-compat",
            ],
            cwd=ROOT_DIR,
            env={
                **cargo_env,
                "LINUX_UART_SERVER_ELF": str(uart_elf),
                "LINUX_VFS_SERVER_ELF": str(vfs_elf),
            },
        )
        require_target_executable_elf(PREFIX, target, host_elf, "linux-compat ELF")

        log(PREFIX, "packing image")
        pack_env = os.environ.copy()
        pack_env["ROOTSERVER_ELF"] = str(host_elf)
        pack_env["OUT_IMAGE"] = str(packed_image)
        pack_env["KERNEL_ROOT_CNODE_SIZE_BITS"] = "16"
        pack_env["ARCH"] = "riscv64"
        run([str(ROOT_DIR / "tools" / "pack-image.py")], cwd=ROOT_DIR, env=pack_env)
    finally:
        lock.release()

    qemu_cmd = [
        *target.qemu_base_cmd(smp, "3072"),
        "-kernel",
        str(packed_image),
        "-chardev",
        f"file,id=kerneldebug,path={kernel_debug_log_file}",
        "-device",
        "pci-serial,chardev=kerneldebug,addr=1",
    ]
    kernel_debug_log_file.parent.mkdir(parents=True, exist_ok=True)
    kernel_debug_log_file.unlink(missing_ok=True)
    log_file.parent.mkdir(parents=True, exist_ok=True)

    log(PREFIX, f"booting linux-compat; log: {log_file}")
    runner = LoggedProcess(qemu_cmd, log_file, verbose=verbose)
    proc = runner.start()
    status = 2
    deadline = time.time() + timeout
    try:
        while time.time() < deadline:
            if proc.poll() is not None:
                break
            if file_has_regex(log_file, SUCCESS_RE):
                status = 0
                break
            if file_has_regex(log_file, FAIL_RE):
                status = 1
                break
            time.sleep(0.2)
    finally:
        runner.terminate()
        runner.close()

    if status == 2 and file_has_regex(log_file, SUCCESS_RE):
        status = 0
    if status == 2 and file_has_regex(log_file, FAIL_RE):
        status = 1

    if status == 0:
        print(f"PASS: {SUCCESS_RE}")
        print(f"      log: {log_file}")
    elif status == 1:
        print("FAIL: linux-compat LTP wave-1 did not complete")
        print(f"      log: {log_file}")
        print("      tail of log:")
        for line in tail_lines(log_file, 40):
            print(f"        {line}")
    else:
        print(f"TIMEOUT after {timeout}s without seeing {SUCCESS_RE}")
        print(f"      log: {log_file}")
        print("      tail of log:")
        for line in tail_lines(log_file, 40):
            print(f"        {line}")
    return status


if __name__ == "__main__":
    raise SystemExit(main())
