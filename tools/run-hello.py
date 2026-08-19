#!/usr/bin/env python3
"""Build, pack, and run hello-rootserver on QEMU."""

from __future__ import annotations

import os
import sys
import time
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parent))
from target_config import (
    platform_from_env,
    rust_target_from_env,
    sel4_arch_from_env,
    strip_from_env,
    target_from_env,
)
from tool_common import (
    ROOT_DIR,
    LoggedProcess,
    command_exists,
    die,
    ensure_rust_log_at_least_info,
    file_has_regex,
    getenv,
    install_file,
    log,
    qemu_smp_arg,
    require_file,
    require_target_executable_elf,
    run,
    tail_lines,
)


PREFIX = "run-hello"
SUCCESS_RE = r"hello-rootserver: ok"
FAIL_RE = r"KERNEL PANIC|hello-rootserver panic|hello-rootserver: missing"


def objcopy_from_strip(strip: str) -> str:
    return os.environ.get("OBJCOPY", strip.removesuffix("strip") + "objcopy")


def rust_kernel_env() -> dict[str, str]:
    env = os.environ.copy()
    env.pop("CARGO_TARGET_DIR", None)
    env["SMP"] = os.environ.get("SMP", "OFF")
    env["NUM_NODES"] = os.environ.get("NUM_NODES", "1")
    env["KERNEL_ROOT_CNODE_SIZE_BITS"] = os.environ.get("KERNEL_ROOT_CNODE_SIZE_BITS", "13")
    return env


def audit_rust_kernel(target, rust_target: str, rust_kernel_elf: Path, env: dict[str, str]) -> None:
    audit_env = env.copy()
    audit_env["ARCH"] = target.name
    audit_env["RUST_TARGET"] = rust_target
    for script in (
        "tools/audit-trap-layout.py",
        "tools/audit-user-context-abi.py",
        "tools/audit-syscall-abi.py",
        "tools/audit-smp-abi.py",
        "tools/audit-platform-abi.py",
        "tools/audit-vspace-abi.py",
    ):
        log(PREFIX, f"running {script}...")
        run([sys.executable, script], cwd=ROOT_DIR, env=audit_env)
    run(
        [sys.executable, "tools/audit-kernel-elf.py", str(rust_kernel_elf)],
        cwd=ROOT_DIR,
        env=audit_env,
    )
    run(
        [
            sys.executable,
            "tools/audit-kernel-fpu.py",
            "--target",
            rust_target,
            str(rust_kernel_elf),
        ],
        cwd=ROOT_DIR,
        env=audit_env,
    )


def main() -> int:
    ensure_rust_log_at_least_info()
    target = target_from_env(PREFIX)
    rust_target = rust_target_from_env(target)
    cargo_env = rust_kernel_env()

    hello_elf = ROOT_DIR / "target" / rust_target / "release" / "hello-rootserver"
    kernel_elf = ROOT_DIR / "target" / rust_target / "release" / "kernel"
    out_dir = ROOT_DIR / "images"
    out_dir.mkdir(parents=True, exist_ok=True)
    log_file = Path(getenv("LOG_FILE", str(ROOT_DIR / "target" / "hello-last-run.log")))
    timeout = int(getenv("TIMEOUT", "60"))
    smp = qemu_smp_arg("1")
    verbose = "--verbose" in sys.argv or "-v" in sys.argv

    log(
        PREFIX,
        (
            f"building kernel and hello-rootserver for ARCH={target.name} "
            f"target={rust_target} SMP={cargo_env['SMP']} NUM_NODES={cargo_env['NUM_NODES']}"
        ),
    )
    run(
        [
            "cargo",
            "build",
            "--release",
            "--target",
            rust_target,
            "-p",
            "kernel",
            "-p",
            "sel4-user",
            "-p",
            "hello-rootserver",
        ],
        cwd=ROOT_DIR,
        env=cargo_env,
    )
    require_file(PREFIX, kernel_elf, f"kernel ELF missing: {kernel_elf}")
    require_file(PREFIX, hello_elf, f"hello-rootserver ELF missing: {hello_elf}")
    require_target_executable_elf(PREFIX, target, hello_elf, "hello-rootserver ELF")
    audit_rust_kernel(target, rust_target, kernel_elf, cargo_env)

    if target.name != "x86_64":
        die(PREFIX, f"run-hello.py is the x86_64 user-bring-up gate; ARCH={target.name} is unsupported")

    strip = strip_from_env(target)
    objcopy = objcopy_from_strip(strip)
    if not command_exists(objcopy):
        die(PREFIX, f"{objcopy} not on PATH; set OBJCOPY for ARCH=x86_64")
    kernel_image = out_dir / f"kernel-{sel4_arch_from_env(target)}-{platform_from_env(target)}"
    tmp_kernel = out_dir / (kernel_image.name + ".tmp")
    run([objcopy, "-O", "elf32-i386", str(kernel_elf), str(tmp_kernel)])
    install_file(tmp_kernel, kernel_image)
    tmp_kernel.unlink(missing_ok=True)
    rootserver_image = out_dir / "hello-rootserver"
    install_file(hello_elf, rootserver_image)
    target.require_qemu(PREFIX)
    cmd = [
        *target.qemu_base_cmd(smp, "512M"),
        "-no-reboot",
        "-kernel",
        str(kernel_image),
        "-initrd",
        str(rootserver_image),
    ]

    log_file.parent.mkdir(parents=True, exist_ok=True)
    runner = LoggedProcess(cmd, log_file, verbose=verbose)
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
        print("FAIL: hello-rootserver did not complete")
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
