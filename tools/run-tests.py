#!/usr/bin/env python3
"""Run the packed sel4test image under QEMU and classify the result."""

from __future__ import annotations

import sys
import time
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parent))
from tool_common import (
    ROOT_DIR,
    LoggedProcess,
    count_regex_lines,
    ensure_rust_log_at_least_info,
    file_has_regex,
    getenv,
    last_regex_line,
    qemu_smp_arg,
    tail_lines,
)
from target_config import image_name_from_env, target_from_env


DEFAULT_EXPECTED_BASELINE = ""
PREFIX = "run-tests"


def usage_error(arg: str) -> int:
    print(f"unknown arg: {arg}", file=sys.stderr)
    return 3


def main(argv: list[str]) -> int:
    ensure_rust_log_at_least_info()
    target = target_from_env(PREFIX)
    image_name = image_name_from_env(target)
    verbose = False
    for arg in argv:
        if arg in ("--verbose", "-v"):
            verbose = True
        else:
            return usage_error(arg)

    packed_image = Path(getenv("OUT_IMAGE", str(ROOT_DIR / "images" / image_name)))
    log_file = Path(getenv("LOG_FILE", str(ROOT_DIR / "target" / "sel4test-last-run.log")))
    kernel_debug_log_file = Path(
        getenv("KERNEL_DEBUG_LOG_FILE", str(ROOT_DIR / "target" / "sel4test-kernel-debug.log"))
    )
    timeout = int(getenv("TIMEOUT", "180"))
    smp = qemu_smp_arg("2")
    expected_baseline = getenv("SEL4TEST_EXPECTED_BASELINE", DEFAULT_EXPECTED_BASELINE)

    if not packed_image.is_file():
        print(f"packed image not found at {packed_image}", file=sys.stderr)
        print("run tools/pack-image.py first", file=sys.stderr)
        return 3
    target.require_qemu(PREFIX)

    cmd = [
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

    runner = LoggedProcess(cmd, log_file, verbose=verbose)
    proc = runner.start()
    status = 2
    deadline = time.time() + timeout
    baseline_re = (
        rf"Test suite failed\. {expected_baseline} tests passed\." if expected_baseline else None
    )

    def test_log_has(pattern: str) -> bool:
        return file_has_regex(kernel_debug_log_file, pattern) or file_has_regex(log_file, pattern)

    try:
        while time.time() < deadline:
            if proc.poll() is not None:
                break
            if test_log_has(r"Test suite passed\."):
                status = 0
                break
            if baseline_re is not None and test_log_has(baseline_re):
                status = 0
                break
            if test_log_has(r"Test suite failed|seL4 root server abort|\*\*\* KERNEL PANIC"):
                status = 1
                break
            time.sleep(0.5)
    finally:
        runner.terminate()
        runner.close()

    if status == 2 and test_log_has(r"Test suite passed\."):
        status = 0
    if status == 2 and baseline_re is not None and test_log_has(baseline_re):
        status = 0

    passed_line = last_regex_line(kernel_debug_log_file, r"Test suite passed\..*tests passed") or last_regex_line(
        log_file, r"Test suite passed\..*tests passed"
    )
    baseline_line = (
        (
            last_regex_line(kernel_debug_log_file, baseline_re)
            or last_regex_line(log_file, baseline_re)
        )
        if baseline_re is not None
        else None
    )
    result_line = (
        passed_line
        or (f"accepted baseline: {baseline_line}" if baseline_line else None)
        or "Test suite passed."
    )
    test_count = count_regex_lines(kernel_debug_log_file, r"^Starting test [0-9]+:")
    if test_count == 0:
        test_count = count_regex_lines(log_file, r"^Starting test [0-9]+:")

    if status == 0:
        print(f"PASS: {result_line}")
        print(f"      log: {log_file}")
        print(f"      kernel debug log: {kernel_debug_log_file}")
    elif status == 1:
        print("FAIL: test suite reported failure or kernel aborted")
        print(f"      saw {test_count} 'Starting test ...' lines")
        print(f"      kernel debug log: {kernel_debug_log_file}")
        print("      tail of log:")
        for line in tail_lines(log_file, 20):
            print(f"        {line}")
        print("      kernel debug tail:")
        for line in tail_lines(kernel_debug_log_file, 20):
            print(f"        {line}")
    else:
        print(f"TIMEOUT after {timeout}s without seeing pass/fail banner")
        print(f"      saw {test_count} 'Starting test ...' lines")
        print(f"      kernel debug log: {kernel_debug_log_file}")
        print("      tail of log:")
        for line in tail_lines(log_file, 20):
            print(f"        {line}")
        print("      kernel debug tail:")
        for line in tail_lines(kernel_debug_log_file, 20):
            print(f"        {line}")
    return status


if __name__ == "__main__":
    raise SystemExit(main(sys.argv[1:]))
