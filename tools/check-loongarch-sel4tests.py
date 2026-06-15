#!/usr/bin/env python3
"""Report whether a LoongArch64-capable sel4test tree is available."""

from __future__ import annotations

import argparse
import sys
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parent))
from target_config import (
    TARGETS,
    sel4_build_dir_from_env,
    sel4_tree_dir_from_env,
)


PREFIX = "check-loongarch-sel4tests"


def format_candidates(paths: tuple[Path, ...]) -> str:
    return " or ".join(str(path) for path in paths)


def print_check(label: str, ok: bool, detail: str) -> None:
    status = "ok" if ok else "missing"
    print(f"[{PREFIX}] {status}: {label}: {detail}")


def main(argv: list[str]) -> int:
    parser = argparse.ArgumentParser(
        description=(
            "Check the seL4/libsel4/elfloader pieces required to build "
            "ARCH=loongarch64 sel4tests for this repository."
        )
    )
    parser.add_argument(
        "--tree",
        type=Path,
        help="LoongArch-capable sel4test tree; defaults to SEL4_TREE_DIR/SEL4_ROOT or vendored tree",
    )
    parser.add_argument(
        "--build-dir",
        type=Path,
        help="sel4test build directory; defaults to SEL4_BUILD_DIR or the target default",
    )
    parser.add_argument(
        "--strict",
        action="store_true",
        help="return non-zero when required LoongArch sel4test pieces are missing",
    )
    args = parser.parse_args(argv)

    target = TARGETS["loongarch64"]
    build_dir = args.build_dir or sel4_build_dir_from_env(target)
    if args.tree is not None:
        tree_dir = args.tree
    else:
        tree_dir = sel4_tree_dir_from_env(build_dir)

    source_dir = tree_dir / "projects" / "sel4test"
    init_build = tree_dir / "init-build.sh"
    status = target.sel4_arch_source_status(tree_dir)

    print(f"[{PREFIX}] target: ARCH=loongarch64 RUST_TARGET={target.rust_target}")
    print(f"[{PREFIX}] sel4 tree: {tree_dir}")
    print(f"[{PREFIX}] build dir: {build_dir}")
    print_check("tree", tree_dir.is_dir(), str(tree_dir))
    print_check("sel4test source", source_dir.is_dir(), str(source_dir))
    print_check("init-build", init_build.is_file(), str(init_build))
    print_check("kernel arch", status.has_kernel_arch, format_candidates(status.kernel_arch_dirs))
    print_check("libsel4 arch", status.has_libsel4_arch, str(status.libsel4_dir))
    print_check(
        "elfloader source",
        status.has_elfloader_src,
        format_candidates(status.elfloader_src_dirs),
    )
    print_check(
        "elfloader include",
        status.has_elfloader_include,
        format_candidates(status.elfloader_include_dirs),
    )

    if status.is_ready and tree_dir.is_dir() and source_dir.is_dir() and init_build.is_file():
        print(f"[{PREFIX}] ready: this tree can be used by the repository sel4test flow")
        print("[{0}] next:".format(PREFIX))
        print(
            f"  SEL4_TREE_DIR={tree_dir} SEL4_BUILD_DIR={build_dir} "
            "ARCH=loongarch64 ./tools/pack-image.py"
        )
        print("  ARCH=loongarch64 ./tools/run-tests.py")
        return 0

    print(f"[{PREFIX}] not ready: no complete LoongArch64 sel4tests tree is available here")
    print(f"[{PREFIX}] missing port pieces:")
    for item in status.missing_descriptions():
        print(f"  - {item}")
    print(f"[{PREFIX}] provide an external LoongArch-capable seL4/sel4test tree with:")
    print("  SEL4_TREE_DIR=/path/to/loongarch64-sel4test")
    print("  SEL4_BUILD_DIR=/path/to/loongarch64-sel4test/build-loongarch64")
    print("  ARCH=loongarch64 ./tools/pack-image.py")
    print("  ARCH=loongarch64 ./tools/run-tests.py")
    return 1 if args.strict else 0


if __name__ == "__main__":
    raise SystemExit(main(sys.argv[1:]))
