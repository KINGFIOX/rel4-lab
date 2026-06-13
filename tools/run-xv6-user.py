#!/usr/bin/env python3
"""Build, pack, and run an xv6 user program under the Rust seL4 stack."""

from __future__ import annotations

import os
import shutil
import subprocess
import sys
import time
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parent))
from tool_common import (
    ROOT_DIR,
    BuildLock,
    LoggedProcess,
    die,
    ensure_rust_log_at_least_info,
    env_flag,
    file_has_regex,
    getenv,
    last_regex_line,
    log,
    qemu_smp_arg,
    tail_lines,
)
from target_config import (
    image_suffix_from_env,
    sel4_build_dir_from_env,
    sel4_tree_dir_from_env,
    target_from_env,
)


PREFIX = "run-xv6-user"


def usage() -> None:
    print(
        """usage: tools/run-xv6-user.py [--verbose|-v] [--expect-timeout] [--stdin TEXT | --stdin-file PATH] [--qemu-stdin TEXT | --qemu-stdin-file PATH] PROGRAM [ARG...]

Examples:
  tools/run-xv6-user.py echo hello from xv6
  tools/run-xv6-user.py --stdin 'echo hi
' sh
  tools/run-xv6-user.py --stdin-file script.py sh
  tools/run-xv6-user.py --expect-timeout --qemu-stdin 'echo hi
' sh
  TIMEOUT=90 tools/run-xv6-user.py --expect-timeout grind
  TIMEOUT=60 tools/run-xv6-user.py sh

Environment:
  RUST_LOG=LEVEL       build Rust kernel/userspace logs at LEVEL, default info
  SMP=N|ON|OFF         QEMU CPU count, or ON to use NUM_NODES, default 2
  XV6_ATTACH_FS_IMG=0  boot without attaching xv6 fs.img as virtio-blk
  XV6_BUILD_FS_IMG=0   attach existing XV6_FS_IMG without rebuilding it
  XV6_FS_IMG=PATH      fs image path, default target/xv6compat/fs.img
  XV6_KEEP_RUN_FS_IMG=1 keep the default per-run fs.img copy after QEMU exits
  XV6_EXPECT_TIMEOUT=1  treat timeout as success if no fatal log pattern appears
  KERNEL_DEBUG_LOG_FILE=PATH  kernel debug UART log path
""",
        file=sys.stderr,
    )


def parse_args(argv: list[str]):
    verbose = False
    expect_timeout = env_flag("XV6_EXPECT_TIMEOUT")
    qemu_stdin_text = ""
    qemu_stdin_file = ""
    passthrough: list[str] = []
    i = 0
    while i < len(argv):
        arg = argv[i]
        if arg in ("--verbose", "-v"):
            verbose = True
            i += 1
        elif arg == "--expect-timeout":
            expect_timeout = True
            i += 1
        elif arg == "--stdin":
            if i + 1 >= len(argv):
                die(PREFIX, "--stdin requires text")
            os.environ["XV6_CONSOLE_INPUT"] = argv[i + 1]
            i += 2
        elif arg == "--stdin-file":
            if i + 1 >= len(argv):
                die(PREFIX, "--stdin-file requires a path")
            path = Path(argv[i + 1])
            if not path.is_file():
                die(PREFIX, f"stdin file not found: {path}")
            os.environ["XV6_CONSOLE_INPUT"] = path.read_text()
            i += 2
        elif arg == "--qemu-stdin":
            if i + 1 >= len(argv):
                die(PREFIX, "--qemu-stdin requires text")
            qemu_stdin_text = argv[i + 1]
            i += 2
        elif arg == "--qemu-stdin-file":
            if i + 1 >= len(argv):
                die(PREFIX, "--qemu-stdin-file requires a path")
            path = Path(argv[i + 1])
            if not path.is_file():
                die(PREFIX, f"qemu stdin file not found: {path}")
            qemu_stdin_file = str(path)
            i += 2
        elif arg in ("--help", "-h"):
            usage()
            raise SystemExit(0)
        else:
            passthrough = argv[i:]
            break
    if not passthrough:
        usage()
        raise SystemExit(2)
    return verbose, expect_timeout, qemu_stdin_text, qemu_stdin_file, passthrough


def check_output_text(cmd: list[str], env: dict[str, str] | None = None) -> str:
    return subprocess.check_output(cmd, text=True, env=env).strip()


def main(argv: list[str]) -> int:
    ensure_rust_log_at_least_info()
    target = target_from_env(PREFIX)
    verbose, expect_timeout, qemu_stdin_text, qemu_stdin_file, program_args = parse_args(argv)
    program = program_args[0].removeprefix("_")
    timeout = int(getenv("TIMEOUT", "30"))
    smp = qemu_smp_arg("2")
    attach_fs_img = getenv("XV6_ATTACH_FS_IMG", "1") == "1"
    build_fs_img = getenv("XV6_BUILD_FS_IMG", "1") == "1"
    fs_img_explicit = "XV6_FS_IMG" in os.environ
    xv6_fs_img = Path(getenv("XV6_FS_IMG", str(ROOT_DIR / "target" / "xv6compat" / "fs.img")))
    keep_run_fs_img = getenv("XV6_KEEP_RUN_FS_IMG", "0") == "1"
    run_id = getenv("XV6_RUN_ID", f"{program}-{os.getpid()}")
    run_fs_img: Path | None = None
    run_qemu_stdin_file: Path | None = None

    if qemu_stdin_text:
        run_qemu_stdin_file = ROOT_DIR / "target" / "xv6compat" / f"qemu-stdin-{run_id}.txt"
        run_qemu_stdin_file.parent.mkdir(parents=True, exist_ok=True)
        run_qemu_stdin_file.write_text(qemu_stdin_text)
        qemu_stdin_file = str(run_qemu_stdin_file)

    target.require_qemu(PREFIX)
    target.require_sel4_arch_source(
        PREFIX,
        sel4_tree_dir_from_env(sel4_build_dir_from_env(target)),
    )

    lock = BuildLock(ROOT_DIR)
    lock.acquire()
    try:
        rootserver_elf = Path(check_output_text([str(ROOT_DIR / "tools" / "build-xv6-user-rootserver.py"), *program_args]))
        image_suffix = image_suffix_from_env(target)
        packed_image = Path(getenv("OUT_IMAGE", str(ROOT_DIR / "images" / f"xv6-{run_id}-{image_suffix}")))
        log_file = Path(getenv("LOG_FILE", str(ROOT_DIR / "target" / f"xv6-{run_id}-last-run.log")))
        kernel_debug_log_file = Path(
            getenv("KERNEL_DEBUG_LOG_FILE", str(ROOT_DIR / "target" / f"xv6-{run_id}-kernel-debug.log"))
        )

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
        lock.release()
    except subprocess.CalledProcessError as exc:
        lock.release()
        if run_fs_img is not None and not keep_run_fs_img:
            run_fs_img.unlink(missing_ok=True)
        if run_qemu_stdin_file is not None:
            run_qemu_stdin_file.unlink(missing_ok=True)
        return exc.returncode
    except Exception:
        lock.release()
        if run_fs_img is not None and not keep_run_fs_img:
            run_fs_img.unlink(missing_ok=True)
        if run_qemu_stdin_file is not None:
            run_qemu_stdin_file.unlink(missing_ok=True)
        raise

    qemu_cmd = [
        *target.qemu_base_cmd(smp, "3072"),
        "-kernel",
        str(packed_image),
        "-chardev",
        f"file,id=kerneldebug,path={kernel_debug_log_file}",
        "-device",
        "pci-serial,chardev=kerneldebug,addr=1",
    ]
    if attach_fs_img:
        qemu_cmd.extend(target.xv6_fs_device_args(xv6_fs_img))

    kernel_debug_log_file.parent.mkdir(parents=True, exist_ok=True)
    kernel_debug_log_file.unlink(missing_ok=True)

    log(PREFIX, f"booting {program}; log: {log_file}")
    runner = LoggedProcess(
        qemu_cmd,
        log_file,
        verbose=verbose,
        stdin_path=Path(qemu_stdin_file) if qemu_stdin_file else None,
        stdin_delay_until=(kernel_debug_log_file, r"uart-server: init complete") if qemu_stdin_file else None,
    )
    proc = runner.start()
    root_exit_re = r"xv6-host: exit\([^)]*\) pid=1([^0-9]|$)"
    root_exit_ok_re = r"xv6-host: exit\(0\) pid=1([^0-9]|$)"
    fatal_re = r"\*\*\* KERNEL PANIC|kernel-mode trap|user fault:"
    expect_timeout_fatal_re = getenv(
        "XV6_EXPECT_TIMEOUT_FATAL_RE",
        fatal_re + r"|xv6-host: fault kill|grind:|panic",
    )
    runtime_fatal_re = expect_timeout_fatal_re if expect_timeout else fatal_re
    status = 2
    deadline = time.time() + timeout

    def has_fatal(pattern: str) -> bool:
        return file_has_regex(kernel_debug_log_file, pattern) or file_has_regex(log_file, pattern)

    def has_runtime_line(pattern: str) -> bool:
        return file_has_regex(kernel_debug_log_file, pattern) or file_has_regex(log_file, pattern)

    def last_runtime_line(pattern: str) -> str:
        line = last_regex_line(kernel_debug_log_file, pattern)
        if line:
            return line
        return last_regex_line(log_file, pattern)

    try:
        while time.time() < deadline:
            if has_runtime_line(root_exit_ok_re):
                status = 0
                break
            if has_runtime_line(root_exit_re):
                status = 1
                break
            if has_fatal(runtime_fatal_re):
                status = 1
                break
            if proc.poll() is not None:
                status = 0 if has_runtime_line(root_exit_ok_re) else 1
                break
            time.sleep(0.2)
    finally:
        runner.terminate()
        runner.close()
        if run_fs_img is not None and not keep_run_fs_img:
            run_fs_img.unlink(missing_ok=True)
        if run_qemu_stdin_file is not None:
            run_qemu_stdin_file.unlink(missing_ok=True)

    if has_runtime_line(root_exit_ok_re):
        status = 0

    if status == 0:
        exit_line = last_runtime_line(root_exit_ok_re)
        print(f"PASS: {exit_line}")
        print(f"      log: {log_file}")
        print(f"      kernel debug log: {kernel_debug_log_file}")
    elif status == 1:
        print("FAIL: xv6 user run aborted")
        print(f"      log: {log_file}")
        print(f"      kernel debug log: {kernel_debug_log_file}")
        print("      tail:")
        for line in tail_lines(log_file, 30):
            print(f"        {line}")
        print("      kernel debug tail:")
        for line in tail_lines(kernel_debug_log_file, 30):
            print(f"        {line}")
    else:
        if expect_timeout and not (
            has_fatal(expect_timeout_fatal_re) or has_runtime_line(root_exit_re)
        ):
            print(f"PASS: timeout after {timeout}s without fatal xv6 output")
            print(f"      log: {log_file}")
            print(f"      kernel debug log: {kernel_debug_log_file}")
            status = 0
        else:
            print(f"TIMEOUT after {timeout}s")
            print(f"      log: {log_file}")
            print(f"      kernel debug log: {kernel_debug_log_file}")
            print("      tail:")
            for line in tail_lines(log_file, 30):
                print(f"        {line}")
            print("      kernel debug tail:")
            for line in tail_lines(kernel_debug_log_file, 30):
                print(f"        {line}")
    return status


if __name__ == "__main__":
    raise SystemExit(main(sys.argv[1:]))
