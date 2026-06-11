#!/usr/bin/env python3
"""Boot the Rust kernel or packed seL4 image under QEMU."""

from __future__ import annotations

import os
import sys
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parent))
from tool_common import ROOT_DIR, getenv, qemu_smp_arg, run


IMAGE_NAME = "sel4test-driver-image-riscv-qemu-riscv-virt"


def main(argv: list[str]) -> int:
    rust_target = getenv("RUST_TARGET", "riscv64imac-unknown-none-elf")
    kernel_elf = ROOT_DIR / "target" / rust_target / "release" / "kernel"
    packed_image = ROOT_DIR / "images" / IMAGE_NAME
    smp = qemu_smp_arg("1")

    mode = os.environ.get("MODE", "")
    if not mode:
        mode = "image" if packed_image.is_file() else "standalone"

    if mode == "standalone":
        if not kernel_elf.is_file():
            print("kernel ELF not found, building...", file=sys.stderr)
            run(["cargo", "build", "--release"], cwd=ROOT_DIR)
        cmd = [
            "qemu-system-riscv64",
            "-machine",
            "virt",
            "-cpu",
            "rv64",
            "-smp",
            smp,
            "-m",
            "128M",
            "-nographic",
            "-bios",
            "none",
            "-kernel",
            str(kernel_elf),
            *argv,
        ]
    elif mode == "image":
        if not packed_image.is_file():
            print(f"packed image not found at {packed_image}", file=sys.stderr)
            print("run tools/pack-image.py first", file=sys.stderr)
            return 1
        cmd = [
            "qemu-system-riscv64",
            "-machine",
            "virt",
            "-cpu",
            "rv64",
            "-smp",
            smp,
            "-m",
            "3072",
            "-nographic",
            "-bios",
            "none",
            "-kernel",
            str(packed_image),
            *argv,
        ]
    else:
        print(f"unknown MODE={mode}", file=sys.stderr)
        return 1

    os.execvp(cmd[0], cmd)
    return 127


if __name__ == "__main__":
    raise SystemExit(main(sys.argv[1:]))
