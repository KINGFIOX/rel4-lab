#!/usr/bin/env python3
"""Build an xv6 user payload and embed it into the xv6-host rootserver."""

from __future__ import annotations

import os
import subprocess
import sys
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parent))
from tool_common import (
    ROOT_DIR,
    BuildLock,
    bare_metal_tool_env,
    c_string_literal,
    die,
    getenv,
    infer_toolprefix,
    install_file,
    log,
    require_dir,
    require_file,
    run,
    xv6_user_cflags,
)


PREFIX = "build-xv6-user"


def usage() -> None:
    print(
        """usage: tools/build-xv6-user-rootserver.py PROGRAM [ARG...]

Examples:
  tools/build-xv6-user-rootserver.py echo hello from xv6
  tools/build-xv6-user-rootserver.py sh
""",
        file=sys.stderr,
    )


def write_argv_source(path: Path, args: list[str]) -> None:
    lines = [
        '#include "kernel/types.h"',
        '#include "user/user.h"',
        "",
        "extern int main(int, char **);",
        "",
    ]
    for i, arg in enumerate(args):
        lines.append(f'static char arg_{i}[] = "{c_string_literal(arg)}";')
    lines.append("static char *compat_argv[] = {" + "".join(f" arg_{i}," for i in range(len(args))) + " 0 };")
    lines.extend(
        [
            "",
            "void _xv6_compat_start(void) {",
            f"  int r = main({len(args)}, compat_argv);",
            "  exit(r);",
            "  for (;;) {}",
            "}",
            "",
        ]
    )
    path.write_text("\n".join(lines))


def write_linker_script(src: Path, dst: Path, user_base: str) -> None:
    text = src.read_text()
    replaced = text.replace(". = 0x0;", f". = {user_base};", 1)
    dst.write_text(replaced)


def main(argv: list[str]) -> int:
    if len(argv) < 1:
        usage()
        return 2

    xv6_dir = Path(getenv("XV6_DIR", str(ROOT_DIR / "third_party" / "xv6-riscv")))
    out_dir = Path(getenv("OUT_DIR", str(ROOT_DIR / "target" / "xv6compat")))
    user_base = getenv("XV6_USER_BASE", "0x10000")
    march = getenv("XV6_USER_MARCH", "rv64imac")
    mabi = getenv("XV6_USER_MABI", "lp64")
    rust_target = getenv("RUST_TARGET", "riscv64imac-unknown-none-elf")

    program = argv[0].removeprefix("_")
    program_args = argv[1:]

    require_dir(PREFIX, xv6_dir, f"XV6_DIR not found: {xv6_dir}")
    require_file(PREFIX, xv6_dir / "user" / f"{program}.c", f"xv6 user program not found: user/{program}.c")

    lock = BuildLock(ROOT_DIR)
    lock.acquire()
    try:
        toolprefix = os.environ.get("TOOLPREFIX") or infer_toolprefix()
        if not toolprefix:
            die(PREFIX, "could not find a RISC-V ELF toolchain")
        cc = f"{toolprefix}gcc"
        ld = f"{toolprefix}ld"
        cross_env = bare_metal_tool_env()

        out_dir.mkdir(parents=True, exist_ok=True)
        cflags = xv6_user_cflags(xv6_dir, march, mabi)

        log(PREFIX, f"building xv6 objects for {program}")
        make_targets = [f"user/{program}.o", "user/ulib.o", "user/usys.o", "user/printf.o", "user/umalloc.o"]
        run(
            [
                "make",
                "-B",
                "-C",
                str(xv6_dir),
                f"TOOLPREFIX={toolprefix}",
                f"CFLAGS={' '.join(cflags)}",
                *make_targets,
            ],
            env=cross_env,
            stdout=subprocess.DEVNULL,
        )

        args_c = out_dir / f"{program}_argv.c"
        args_o = out_dir / f"{program}_argv.o"
        payload_elf = out_dir / f"_{program}-payload"
        host_elf = out_dir / f"xv6-host-{program}-rootserver"
        host_build_elf = ROOT_DIR / "target" / rust_target / "release" / "xv6-host"
        uart_server_elf = ROOT_DIR / "target" / rust_target / "release" / "uart-server"
        vfs_server_elf = ROOT_DIR / "target" / rust_target / "release" / "vfs-server"
        xv6fs_server_elf = ROOT_DIR / "target" / rust_target / "release" / "xv6fs-server"
        disk_server_elf = ROOT_DIR / "target" / rust_target / "release" / "virtio-disk-server"
        linker_script = out_dir / f"user-{user_base}.ld"

        write_argv_source(args_c, [program, *program_args])
        run([cc, *cflags, "-c", "-o", str(args_o), str(args_c)], env=cross_env)
        write_linker_script(xv6_dir / "user" / "user.ld", linker_script, user_base)

        log(PREFIX, f"linking payload {payload_elf}")
        run(
            [
                ld,
                "-z",
                "max-page-size=4096",
                "-T",
                str(linker_script),
                "-e",
                "_xv6_compat_start",
                "-o",
                str(payload_elf),
                str(args_o),
                str(xv6_dir / "user" / f"{program}.o"),
                str(xv6_dir / "user" / "ulib.o"),
                str(xv6_dir / "user" / "usys.o"),
                str(xv6_dir / "user" / "printf.o"),
                str(xv6_dir / "user" / "umalloc.o"),
            ],
            env=cross_env,
        )

        log(PREFIX, f"building xv6-host rootserver {host_elf}")
        run(
            [
                "cargo",
                "build",
                "--manifest-path",
                str(ROOT_DIR / "Cargo.toml"),
                "--release",
                "--target",
                rust_target,
                "-p",
                "uart-server",
                "-p",
                "vfs-server",
                "-p",
                "xv6fs-server",
                "-p",
                "virtio-disk-server",
            ]
        )

        env = os.environ.copy()
        env.update(
            {
                "XV6_PAYLOAD_ELF": str(payload_elf),
                "XV6_UART_SERVER_ELF": str(uart_server_elf),
                "XV6_VFS_SERVER_ELF": str(vfs_server_elf),
                "XV6_XV6FS_SERVER_ELF": str(xv6fs_server_elf),
                "XV6_DISK_SERVER_ELF": str(disk_server_elf),
                "XV6_PAYLOAD_PROGRAM": program,
                "XV6_ROOT_IS_INIT": "1" if program == "init" else "0",
            }
        )
        run(
            [
                "cargo",
                "build",
                "--manifest-path",
                str(ROOT_DIR / "Cargo.toml"),
                "--release",
                "--target",
                rust_target,
                "-p",
                "xv6-host",
            ],
            env=env,
        )
        install_file(host_build_elf, host_elf)
        print(host_elf)
        return 0
    finally:
        lock.release()


if __name__ == "__main__":
    raise SystemExit(main(sys.argv[1:]))
