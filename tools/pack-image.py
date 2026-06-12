#!/usr/bin/env python3
"""Pack the Rust kernel into the upstream seL4 elfloader image."""

from __future__ import annotations

import filecmp
import os
import shutil
import sys
import tempfile
import time
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parent))
from tool_common import (
    ROOT_DIR,
    bare_metal_tool_env,
    die,
    getenv,
    install_file,
    log,
    remove_files,
    require_dir,
    require_file,
    run,
    touch,
)


PREFIX = "pack-image"
IMAGE_NAME = "sel4test-driver-image-riscv-qemu-riscv-virt"
DEFAULT_SEL4_TREE_DIR = ROOT_DIR / "third_party" / "sel4-lab" / "sel4test"
DEFAULT_SEL4_BUILD_DIR = DEFAULT_SEL4_TREE_DIR / "build-riscv64"

PRESERVED_CMAKE_KEYS = (
    "PLATFORM",
    "KernelSel4Arch",
    "SIMULATION",
    "SMP",
    "NUM_NODES",
    "MCS",
    "DOMAINS",
    "ARM_HYP",
    "RELEASE",
    "VERIFICATION",
    "BAMBOO",
    "LibSel4TestPrinterRegex",
    "LibSel4TestPrinterHaltOnTestFailure",
    "KernelTimerTickMS",
    "KernelTimeSlice",
    "LibPlatSupportHaveTimer",
    "Sel4testHaveTimer",
    "Sel4testAllowSettingsOverride",
    "PYTHON3",
)

DEFAULT_CMAKE_DEFS = {
    "PLATFORM": "qemu-riscv-virt",
    "KernelSel4Arch": "riscv64",
    "SIMULATION": "ON",
    "KernelRiscvExtD": "ON",
    "KernelRiscvExtF": "ON",
    "MCS": "ON",
    "SMP": "ON",
    "NUM_NODES": "2",
    "LibSel4TestPrinterRegex": ".*",
}


def make_temp(prefix: str) -> Path:
    fd, name = tempfile.mkstemp(prefix=prefix)
    os.close(fd)
    return Path(name)


def normalize_path(path: Path) -> str:
    return str(path.expanduser().resolve())


def cmake_bool(value: str) -> str:
    return "ON" if value.strip().upper() in ("1", "ON", "TRUE", "YES", "Y") else "OFF"


def cmake_smp(value: str) -> str:
    value = value.strip()
    if value.isdecimal():
        return "ON" if int(value) > 1 else "OFF"
    return cmake_bool(value)


def num_nodes_from_smp(value: str) -> str | None:
    value = value.strip()
    if value.isdecimal():
        return str(max(1, int(value)))
    if value.upper() in ("OFF", "FALSE", "NO", "N", "0"):
        return "1"
    return None


def read_cmake_cache(cache_path: Path) -> dict[str, str]:
    values: dict[str, str] = {}
    try:
        with cache_path.open("r", errors="replace") as f:
            for line in f:
                line = line.rstrip("\n")
                if not line or line.startswith("#") or line.startswith("//") or "=" not in line:
                    continue
                key_and_type, value = line.split("=", 1)
                key = key_and_type.split(":", 1)[0]
                values[key] = value
    except FileNotFoundError:
        pass
    return values


def sel4_tree_dir_for(build_dir: Path) -> Path:
    explicit = os.environ.get("SEL4_TREE_DIR") or os.environ.get("SEL4_ROOT")
    if explicit:
        return Path(explicit)
    if (build_dir.parent / "init-build.sh").is_file():
        return build_dir.parent
    return DEFAULT_SEL4_TREE_DIR


def cache_needs_reconfigure(build_dir: Path, source_dir: Path, cache: dict[str, str]) -> bool:
    if not (build_dir / "build.ninja").is_file():
        return True
    if not cache:
        return True

    expected_source = normalize_path(source_dir)
    cached_source = cache.get("CMAKE_HOME_DIRECTORY", "")
    if not cached_source or normalize_path(Path(cached_source)) != expected_source:
        return True

    cached_build = cache.get("CMAKE_CACHEFILE_DIR", "")
    if cached_build and normalize_path(Path(cached_build)) != normalize_path(build_dir):
        return True

    try:
        ninja_text = (build_dir / "build.ninja").read_text(errors="replace")
    except FileNotFoundError:
        return True
    return expected_source not in ninja_text or normalize_path(build_dir) not in ninja_text


def cache_env_overrides_differ(build_dir: Path, cache: dict[str, str]) -> bool:
    if "MCS" not in os.environ and cache.get("MCS") != DEFAULT_CMAKE_DEFS["MCS"]:
        return True
    for key in ("KernelRiscvExtD", "KernelRiscvExtF"):
        if cache.get(key) != DEFAULT_CMAKE_DEFS[key]:
            return True
    if "SMP" in os.environ and cache.get("SMP") != cmake_smp(os.environ["SMP"]):
        return True
    if "SMP" in os.environ and "NUM_NODES" not in os.environ:
        expected_nodes = num_nodes_from_smp(os.environ["SMP"])
        if expected_nodes is not None and cache.get("NUM_NODES") != expected_nodes:
            return True
    for key in ("SIMULATION", "MCS", "DOMAINS", "ARM_HYP", "RELEASE", "VERIFICATION", "BAMBOO"):
        if key in os.environ and cache.get(key) != cmake_bool(os.environ[key]):
            return True
    if "NUM_NODES" in os.environ and cache.get("NUM_NODES") != os.environ["NUM_NODES"]:
        return True
    expected_regex = os.environ.get("SEL4TEST_REGEX", DEFAULT_CMAKE_DEFS["LibSel4TestPrinterRegex"])
    if cache.get("LibSel4TestPrinterRegex") != expected_regex:
        return True
    if "QEMU_DTB" in os.environ:
        qemu_dtb = Path(os.environ["QEMU_DTB"])
        if qemu_dtb.is_file() and cache.get("QEMU_DTB") != str(qemu_dtb):
            return True
    elif (build_dir / "qemu-riscv-virt.dtb").is_file() and cache.get("QEMU_DTB") != str(
        build_dir / "qemu-riscv-virt.dtb"
    ):
        return True
    return False


def clear_cmake_state(build_dir: Path) -> None:
    for name in ("CMakeCache.txt", "build.ninja", "cmake_install.cmake"):
        try:
            (build_dir / name).unlink()
        except FileNotFoundError:
            pass
    shutil.rmtree(build_dir / "CMakeFiles", ignore_errors=True)


def effective_cmake_values(build_dir: Path, cache: dict[str, str]) -> dict[str, str]:
    values = dict(DEFAULT_CMAKE_DEFS)
    for key in PRESERVED_CMAKE_KEYS:
        if key == "MCS" and "MCS" not in os.environ:
            continue
        if key == "LibSel4TestPrinterRegex" and "SEL4TEST_REGEX" not in os.environ:
            continue
        value = cache.get(key)
        if value:
            values[key] = value

    if "SMP" in os.environ:
        values["SMP"] = cmake_smp(os.environ["SMP"])
        if "NUM_NODES" not in os.environ:
            nodes = num_nodes_from_smp(os.environ["SMP"])
            if nodes is not None:
                values["NUM_NODES"] = nodes
    for key in ("SIMULATION", "MCS", "DOMAINS", "ARM_HYP", "RELEASE", "VERIFICATION", "BAMBOO"):
        if key in os.environ:
            values[key] = cmake_bool(os.environ[key])
    if "NUM_NODES" in os.environ:
        values["NUM_NODES"] = os.environ["NUM_NODES"]
    if "SEL4TEST_REGEX" in os.environ:
        values["LibSel4TestPrinterRegex"] = os.environ["SEL4TEST_REGEX"]

    qemu_dtb = Path(os.environ.get("QEMU_DTB", str(build_dir / "qemu-riscv-virt.dtb")))
    if qemu_dtb.is_file():
        values["QEMU_DTB"] = str(qemu_dtb)

    return values


def cmake_defs(build_dir: Path, cache: dict[str, str]) -> list[str]:
    values = effective_cmake_values(build_dir, cache)
    return [f"-D{key}={value}" for key, value in values.items()]


def rust_kernel_env(build_dir: Path) -> dict[str, str]:
    cache = read_cmake_cache(build_dir / "CMakeCache.txt")
    values = effective_cmake_values(build_dir, cache)
    env = os.environ.copy()
    env["SMP"] = values["SMP"]
    env["NUM_NODES"] = values["NUM_NODES"]
    return env


def ensure_sel4_configured(build_dir: Path) -> None:
    tree_dir = sel4_tree_dir_for(build_dir)
    source_dir = tree_dir / "projects" / "sel4test"
    init_build = tree_dir / "init-build.sh"
    cache_path = build_dir / "CMakeCache.txt"
    cache = read_cmake_cache(cache_path)

    require_dir(PREFIX, tree_dir, f"SEL4_TREE_DIR not found: {tree_dir}")
    require_dir(PREFIX, source_dir, f"sel4test source directory not found: {source_dir}")
    require_file(PREFIX, init_build, f"init-build.sh not found: {init_build}")

    if not cache_needs_reconfigure(build_dir, source_dir, cache) and not cache_env_overrides_differ(
        build_dir, cache
    ):
        return

    log(PREFIX, f"reconfiguring upstream sel4test build in {build_dir}")
    build_dir.mkdir(parents=True, exist_ok=True)
    clear_cmake_state(build_dir)
    run([str(init_build), *cmake_defs(build_dir, cache)], cwd=build_dir)


def main() -> int:
    sel4_build_dir = Path(getenv("SEL4_BUILD_DIR", str(DEFAULT_SEL4_BUILD_DIR)))
    rust_target = getenv("RUST_TARGET", "riscv64gc-unknown-none-elf")
    rust_kernel_elf = ROOT_DIR / "target" / rust_target / "release" / "kernel"
    rootserver_elf_env = os.environ.get("ROOTSERVER_ELF", "")
    rootserver_elf = Path(rootserver_elf_env) if rootserver_elf_env else None
    out_dir = ROOT_DIR / "images"
    out_image = Path(getenv("OUT_IMAGE", str(out_dir / IMAGE_NAME)))
    strip = getenv("STRIP", "riscv64-none-elf-strip")
    cross_env = bare_metal_tool_env()

    tmp_stripped = make_temp("rust-kernel.elf.")
    tmp_rootserver_stripped: Path | None = None
    try:
        cargo_env = rust_kernel_env(sel4_build_dir)
        log(
            PREFIX,
            f"building Rust kernel (SMP={cargo_env['SMP']} NUM_NODES={cargo_env['NUM_NODES']})...",
        )
        run(
            ["cargo", "build", "--release", "--target", rust_target, "-p", "kernel"],
            cwd=ROOT_DIR,
            env=cargo_env,
        )
        require_file(PREFIX, rust_kernel_elf, f"Rust kernel ELF missing: {rust_kernel_elf}")

        require_dir(PREFIX, sel4_build_dir, f"SEL4_BUILD_DIR not found: {sel4_build_dir}")
        ensure_sel4_configured(sel4_build_dir)
        if rootserver_elf is not None:
            require_file(PREFIX, rootserver_elf, f"ROOTSERVER_ELF not found: {rootserver_elf}")

        if rootserver_elf is None:
            remove_files([sel4_build_dir / "elfloader" / "rootserver"])

        log(PREFIX, "refreshing upstream image prerequisites...")
        run(["ninja", f"images/{IMAGE_NAME}"], cwd=sel4_build_dir, env=cross_env)
        require_file(
            PREFIX,
            sel4_build_dir / "kernel" / "kernel.dtb",
            "kernel/kernel.dtb missing after upstream refresh",
        )
        require_file(
            PREFIX,
            sel4_build_dir / "elfloader" / "rootserver",
            "elfloader/rootserver missing after upstream refresh",
        )

        log(PREFIX, "installing Rust kernel into elfloader staging...")
        run([strip, str(rust_kernel_elf), "-o", str(tmp_stripped)])
        install_file(tmp_stripped, sel4_build_dir / "elfloader" / "kernel.elf")

        if rootserver_elf is not None:
            log(PREFIX, f"installing custom rootserver: {rootserver_elf}")
            tmp_rootserver_stripped = make_temp("rootserver.elf.")
            run([strip, str(rootserver_elf), "-o", str(tmp_rootserver_stripped)])
            remove_files([sel4_build_dir / "elfloader" / "rootserver"])
            install_file(tmp_rootserver_stripped, sel4_build_dir / "elfloader" / "rootserver")

        time.sleep(1)
        touch(sel4_build_dir / "elfloader" / "kernel.elf")
        if rootserver_elf is not None:
            touch(sel4_build_dir / "elfloader" / "rootserver")

        log(PREFIX, "invalidating downstream artifacts...")
        remove_files(
            [
                sel4_build_dir / "elfloader" / "archive.archive.o.cpio",
                sel4_build_dir / "elfloader" / "archive.o",
                sel4_build_dir / "elfloader" / "elfloader",
                sel4_build_dir / "images" / IMAGE_NAME,
            ]
        )

        log(PREFIX, "running ninja to re-pack image...")
        run(["ninja", f"images/{IMAGE_NAME}"], cwd=sel4_build_dir, env=cross_env)

        if not filecmp.cmp(tmp_stripped, sel4_build_dir / "elfloader" / "kernel.elf", shallow=False):
            die(PREFIX, "elfloader/kernel.elf was overwritten after Rust injection")
        if rootserver_elf is not None and tmp_rootserver_stripped is not None:
            if not filecmp.cmp(
                tmp_rootserver_stripped,
                sel4_build_dir / "elfloader" / "rootserver",
                shallow=False,
            ):
                die(PREFIX, "elfloader/rootserver was overwritten after custom rootserver injection")

        install_file(sel4_build_dir / "images" / IMAGE_NAME, out_image)
        log(PREFIX, f"image ready: {out_image}")
        return 0
    finally:
        remove_files([tmp_stripped])
        if tmp_rootserver_stripped is not None:
            remove_files([tmp_rootserver_stripped])


if __name__ == "__main__":
    raise SystemExit(main())
