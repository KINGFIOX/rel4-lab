#!/usr/bin/env python3
"""Build xv6's native fs.img for the xv6 compatibility path."""

from __future__ import annotations

import os
import shutil
import subprocess
import sys
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parent))
from tool_common import (
    ROOT_DIR,
    BuildLock,
    bare_metal_tool_env,
    die,
    getenv,
    install_file,
    log,
    require_dir,
    run,
    xv6_user_cflags,
)
from target_config import infer_toolprefix_for, target_from_env


PREFIX = "build-xv6-fs-img"


def main() -> int:
    target = target_from_env(PREFIX)
    xv6_dir = Path(getenv("XV6_DIR", str(ROOT_DIR / "third_party" / target.xv6_dir_name)))
    out_dir = Path(getenv("OUT_DIR", str(ROOT_DIR / "target" / "xv6compat")))
    xv6_fs_img = Path(getenv("XV6_FS_IMG", str(out_dir / "fs.img")))
    march = getenv("XV6_USER_MARCH", target.xv6_march)
    mabi = getenv("XV6_USER_MABI", target.xv6_mabi)

    require_dir(PREFIX, xv6_dir, f"XV6_DIR not found: {xv6_dir}")

    lock = BuildLock(ROOT_DIR)
    lock.acquire()
    try:
        toolprefix = os.environ.get("TOOLPREFIX") or infer_toolprefix_for(target)
        if not toolprefix:
            die(PREFIX, f"could not find a {target.name} ELF toolchain")
        host_cc = os.environ.get("HOST_CC") or shutil.which("cc") or shutil.which("clang") or shutil.which("gcc")
        if not host_cc:
            die(PREFIX, "could not find a host C compiler for mkfs")

        cflags = xv6_user_cflags(
            xv6_dir,
            march,
            mabi,
            include_dot=True,
            code_model="medany" if target.name == "riscv64" else None,
        )
        cross_env = bare_metal_tool_env()

        log(PREFIX, "building host mkfs")
        run(
            [
                host_cc,
                "-Wno-unknown-attributes",
                f"-I{xv6_dir}",
                "-o",
                str(xv6_dir / "mkfs" / "mkfs"),
                str(xv6_dir / "mkfs" / "mkfs.c"),
            ]
        )

        log(PREFIX, "building xv6 fs.img")
        run(
            [
                "make",
                "-C",
                str(xv6_dir),
                f"TOOLPREFIX={toolprefix}",
                f"CFLAGS={' '.join(cflags)}",
                "fs.img",
            ],
            env=cross_env,
            stdout=subprocess.DEVNULL,
        )

        install_file(xv6_dir / "fs.img", xv6_fs_img)
        log(PREFIX, f"fs image ready: {xv6_fs_img}")
        print(xv6_fs_img)
        return 0
    finally:
        lock.release()


if __name__ == "__main__":
    raise SystemExit(main())
