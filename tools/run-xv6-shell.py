#!/usr/bin/env python3
"""Build, pack, and boot an interactive xv6 shell under QEMU."""

from __future__ import annotations

import os
import shutil
import signal
import subprocess
import sys
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parent))
from tool_common import (
    ROOT_DIR,
    BuildLock,
    die,
    ensure_rust_log_at_least_info,
    getenv,
    log,
    output,
    qemu_smp_arg,
)
from target_config import image_suffix_from_env, target_from_env


PREFIX = "run-xv6-shell"


def usage() -> None:
    print(
        """usage: tools/run-xv6-shell.py [--no-tty-check]

Builds the xv6-host rootserver with `sh` as the initial user program, packs the
seL4 image, and runs QEMU with serial0 attached directly to the current terminal.

Use Ctrl-a x to quit QEMU.

Environment:
  RUST_LOG=LEVEL        build Rust kernel/userspace logs at LEVEL, default info
  SMP=N|ON|OFF          QEMU CPU count, or ON to use NUM_NODES, default 2
  XV6_ATTACH_FS_IMG=0   boot without attaching xv6 fs.img as virtio-blk
  XV6_BUILD_FS_IMG=0    attach existing XV6_FS_IMG without rebuilding it
  XV6_FS_IMG=PATH       fs image path, default target/xv6compat/fs.img
  XV6_KEEP_RUN_FS_IMG=1 keep the default per-run fs.img copy after QEMU exits
  XV6_RUN_ID=NAME       output image/log suffix, default shell-$PID
  OUT_IMAGE=PATH        packed image path
  KERNEL_DEBUG_LOG_FILE=PATH  kernel debug UART log path
""",
        file=sys.stderr,
    )


def parse_args(argv: list[str]) -> bool:
    no_tty_check = False
    for arg in argv:
        if arg == "--no-tty-check":
            no_tty_check = True
        elif arg in ("--help", "-h"):
            usage()
            raise SystemExit(0)
        else:
            usage()
            die(PREFIX, f"unknown arg: {arg}", code=2)
    return no_tty_check


def check_output_text(cmd: list[str], env: dict[str, str] | None = None) -> str:
    return output(cmd, env=env).strip()


def build_image(run_id: str, target) -> tuple[Path, Path | None, bool]:
    attach_fs_img = getenv("XV6_ATTACH_FS_IMG", "1") == "1"
    build_fs_img = getenv("XV6_BUILD_FS_IMG", "1") == "1"
    fs_img_explicit = "XV6_FS_IMG" in os.environ
    xv6_fs_img = Path(getenv("XV6_FS_IMG", str(ROOT_DIR / "target" / "xv6compat" / "fs.img")))
    keep_run_fs_img = getenv("XV6_KEEP_RUN_FS_IMG", "0") == "1"
    run_fs_img: Path | None = None

    lock = BuildLock(ROOT_DIR)
    lock.acquire()
    try:
        rootserver_elf = Path(
            check_output_text([str(ROOT_DIR / "tools" / "build-xv6-user-rootserver.py"), "sh"])
        )
        image_suffix = image_suffix_from_env(target)
        packed_image = Path(getenv("OUT_IMAGE", str(ROOT_DIR / "images" / f"xv6-{run_id}-{image_suffix}")))

        if attach_fs_img and build_fs_img:
            env = os.environ.copy()
            env["XV6_FS_IMG"] = str(xv6_fs_img)
            xv6_fs_img = Path(check_output_text([str(ROOT_DIR / "tools" / "build-xv6-fs-img.py")], env=env))
        if attach_fs_img and not xv6_fs_img.is_file():
            die(PREFIX, f"XV6_FS_IMG not found: {xv6_fs_img}")
        if attach_fs_img and not fs_img_explicit:
            run_fs_img = ROOT_DIR / "target" / "xv6compat" / f"fs-{run_id}.img"
            run_fs_img.parent.mkdir(parents=True, exist_ok=True)
            shutil.copyfile(xv6_fs_img, run_fs_img)
            xv6_fs_img = run_fs_img

        log(PREFIX, "packing image")
        env = os.environ.copy()
        env["ROOTSERVER_ELF"] = str(rootserver_elf)
        env["OUT_IMAGE"] = str(packed_image)
        subprocess.run([str(ROOT_DIR / "tools" / "pack-image.py")], env=env, check=True)
        return packed_image, xv6_fs_img if attach_fs_img else None, keep_run_fs_img
    except Exception:
        if run_fs_img is not None and not keep_run_fs_img:
            run_fs_img.unlink(missing_ok=True)
        raise
    finally:
        lock.release()


def qemu_command(packed_image: Path, xv6_fs_img: Path | None, kernel_debug_log_file: Path, target) -> list[str]:
    cmd = [
        *target.qemu_base_cmd(qemu_smp_arg("2"), "3072"),
        "-kernel",
        str(packed_image),
        "-chardev",
        f"file,id=kerneldebug,path={kernel_debug_log_file}",
        "-device",
        "pci-serial,chardev=kerneldebug,addr=1",
    ]
    if xv6_fs_img is not None:
        cmd.extend(
            [
                "-global",
                "virtio-mmio.force-legacy=false",
                "-drive",
                f"file={xv6_fs_img},if=none,format=raw,id=xv6fs",
                "-device",
                "virtio-blk-device,drive=xv6fs,bus=virtio-mmio-bus.0",
            ]
        )
    return cmd


def run_interactive(qemu_cmd: list[str]) -> int:
    old_sigint = signal.getsignal(signal.SIGINT)
    proc = subprocess.Popen(qemu_cmd)
    try:
        signal.signal(signal.SIGINT, signal.SIG_IGN)
        return proc.wait()
    finally:
        signal.signal(signal.SIGINT, old_sigint)
        if proc.poll() is None:
            proc.terminate()
            try:
                proc.wait(timeout=2)
            except subprocess.TimeoutExpired:
                proc.kill()
                proc.wait()


def main(argv: list[str]) -> int:
    ensure_rust_log_at_least_info()
    target = target_from_env(PREFIX)
    no_tty_check = parse_args(argv)

    target.require_qemu(PREFIX)
    if not no_tty_check and (not sys.stdin.isatty() or not sys.stdout.isatty()):
        die(PREFIX, "interactive shell needs a terminal; rerun from a real tty or pass --no-tty-check")

    run_id = getenv("XV6_RUN_ID", f"shell-{os.getpid()}")
    kernel_debug_log_file = Path(
        getenv("KERNEL_DEBUG_LOG_FILE", str(ROOT_DIR / "target" / f"xv6-{run_id}-kernel-debug.log"))
    )
    kernel_debug_log_file.parent.mkdir(parents=True, exist_ok=True)
    kernel_debug_log_file.unlink(missing_ok=True)

    xv6_fs_img: Path | None = None
    keep_run_fs_img = True
    try:
        packed_image, xv6_fs_img, keep_run_fs_img = build_image(run_id, target)
        qemu_cmd = qemu_command(packed_image, xv6_fs_img, kernel_debug_log_file, target)

        log(PREFIX, "booting interactive xv6 shell")
        log(PREFIX, "QEMU serial0 is attached to this terminal")
        log(PREFIX, "quit QEMU with Ctrl-a x")
        log(PREFIX, f"kernel debug log: {kernel_debug_log_file}")
        return run_interactive(qemu_cmd)
    finally:
        if xv6_fs_img is not None and not keep_run_fs_img and "XV6_FS_IMG" not in os.environ:
            xv6_fs_img.unlink(missing_ok=True)


if __name__ == "__main__":
    raise SystemExit(main(sys.argv[1:]))
