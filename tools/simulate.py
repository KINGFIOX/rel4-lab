#!/usr/bin/env python3
"""Boot the Rust kernel or packed seL4 image under QEMU."""

from __future__ import annotations

import os
import sys
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parent))
from tool_common import ROOT_DIR, getenv, qemu_smp_arg, run
from target_config import (
    image_name_from_env,
    rust_target_from_env,
    sel4_build_dir_from_env,
    sel4_tree_dir_from_env,
    target_from_env,
)


def main(argv: list[str]) -> int:
    target = target_from_env("simulate")
    rust_target = rust_target_from_env(target)
    kernel_elf = ROOT_DIR / "target" / rust_target / "release" / "kernel"
    packed_image = Path(getenv("OUT_IMAGE", str(ROOT_DIR / "images" / image_name_from_env(target))))
    smp = qemu_smp_arg("1")

    mode = os.environ.get("MODE", "")
    if not mode:
        mode = "image" if packed_image.is_file() else "standalone"

    if mode == "standalone":
        if not kernel_elf.is_file():
            print("kernel ELF not found, building...", file=sys.stderr)
            run(["cargo", "build", "--release", "--target", rust_target, "-p", "kernel"], cwd=ROOT_DIR)
        cmd = [
            *target.qemu_base_cmd(smp, "128M"),
            "-kernel",
            str(kernel_elf),
            *argv,
        ]
    elif mode == "image":
        if not packed_image.is_file():
            target.require_sel4_arch_source(
                "simulate",
                sel4_tree_dir_from_env(sel4_build_dir_from_env(target)),
            )
            print(f"packed image not found at {packed_image}", file=sys.stderr)
            print("run tools/pack-image.py first", file=sys.stderr)
            return 1
        cmd = [
            *target.qemu_base_cmd(smp, "3072"),
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
