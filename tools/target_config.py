"""Target architecture configuration shared by repository tools."""

from __future__ import annotations

import os
from dataclasses import dataclass
from pathlib import Path
from typing import Sequence

from tool_common import ROOT_DIR, command_exists, die


@dataclass(frozen=True)
class TargetConfig:
    name: str
    rust_target: str
    sel4_arch: str
    sel4_source_arch: str
    platform: str
    image_name: str
    default_sel4_build_dir: Path
    strip: str
    qemu: str
    qemu_machine: str
    qemu_cpu: str | None
    xv6_dir_name: str
    xv6_toolprefixes: tuple[str, ...]
    xv6_march: str
    xv6_mabi: str

    def qemu_base_cmd(self, smp: str, memory: str) -> list[str]:
        cmd = [
            self.qemu,
            "-machine",
            self.qemu_machine,
        ]
        if self.qemu_cpu is not None:
            cmd.extend(["-cpu", self.qemu_cpu])
        cmd.extend(
            [
                "-smp",
                smp,
                "-m",
                memory,
                "-nographic",
                "-bios",
                "none",
            ]
        )
        return cmd

    def require_qemu(self, prefix: str) -> None:
        if not command_exists(self.qemu):
            die(prefix, f"{self.qemu} not on PATH; activate the flake dev shell")

    def require_sel4_arch_source(self, prefix: str, sel4_tree_dir: Path) -> None:
        arch_candidates = (
            self.sel4_source_arch,
            self.sel4_arch,
        )
        arch_dirs = [
            sel4_tree_dir / "kernel" / "src" / "arch" / arch
            for arch in dict.fromkeys(arch_candidates)
        ]
        libsel4_dir = (
            sel4_tree_dir
            / "kernel"
            / "libsel4"
            / "sel4_arch_include"
            / self.sel4_arch
        )
        if any(arch_dir.is_dir() for arch_dir in arch_dirs) and libsel4_dir.is_dir():
            return
        arch_list = " or ".join(str(path.relative_to(sel4_tree_dir)) for path in arch_dirs)
        die(
            prefix,
            (
                f"official sel4test for ARCH={self.name} is not available in {sel4_tree_dir}; "
                f"missing {arch_list} and/or "
                f"{libsel4_dir.relative_to(sel4_tree_dir)}. Add a LoongArch seL4/libsel4/"
                "elfloader port or set SEL4_TREE_DIR to one before packing."
            ),
        )


DEFAULT_SEL4_TREE_DIR = ROOT_DIR / "third_party" / "sel4-lab" / "sel4test"

TARGETS: dict[str, TargetConfig] = {
    "riscv64": TargetConfig(
        name="riscv64",
        rust_target="riscv64gc-unknown-none-elf",
        sel4_arch="riscv64",
        sel4_source_arch="riscv",
        platform="qemu-riscv-virt",
        image_name="sel4test-driver-image-riscv-qemu-riscv-virt",
        default_sel4_build_dir=DEFAULT_SEL4_TREE_DIR / "build-riscv64",
        strip="riscv64-none-elf-strip",
        qemu="qemu-system-riscv64",
        qemu_machine="virt",
        qemu_cpu="rv64",
        xv6_dir_name="xv6-riscv",
        xv6_toolprefixes=(
            "riscv64-none-elf-",
            "riscv64-unknown-elf-",
            "riscv64-elf-",
            "riscv64-linux-gnu-",
            "riscv64-unknown-linux-gnu-",
        ),
        xv6_march="rv64gc",
        xv6_mabi="lp64",
    ),
    "loongarch64": TargetConfig(
        name="loongarch64",
        rust_target="loongarch64-unknown-none",
        sel4_arch="loongarch64",
        sel4_source_arch="loongarch",
        platform="qemu-loongarch64-virt",
        image_name="sel4test-driver-image-loongarch64-qemu-loongarch64-virt",
        default_sel4_build_dir=DEFAULT_SEL4_TREE_DIR / "build-loongarch64",
        strip="loongarch64-unknown-linux-gnu-strip",
        qemu="qemu-system-loongarch64",
        qemu_machine="virt",
        qemu_cpu=None,
        xv6_dir_name="xv6-loongarch64",
        xv6_toolprefixes=(
            "loongarch64-none-elf-",
            "loongarch64-unknown-none-",
            "loongarch64-unknown-linux-gnu-",
            "loongarch64-linux-gnu-",
        ),
        xv6_march="loongarch64",
        xv6_mabi="lp64d",
    ),
}


def normalize_arch(value: str) -> str:
    normalized = value.strip().lower().replace("_", "-")
    if normalized in ("", "riscv", "riscv64", "rv64"):
        return "riscv64"
    if normalized in ("loongarch", "loongarch64", "la64"):
        return "loongarch64"
    return normalized


def arch_from_env() -> str:
    arch = os.environ.get("ARCH", "")
    if arch:
        return normalize_arch(arch)

    rust_target = os.environ.get("RUST_TARGET", "")
    if rust_target.startswith("loongarch64-"):
        return "loongarch64"
    return "riscv64"


def target_from_env(prefix: str = "target") -> TargetConfig:
    arch = arch_from_env()
    target = TARGETS.get(arch)
    if target is None:
        die(prefix, f"unsupported ARCH={arch}; supported: {', '.join(sorted(TARGETS))}")
    return target


def rust_target_from_env(target: TargetConfig) -> str:
    return os.environ.get("RUST_TARGET", target.rust_target)


def sel4_build_dir_from_env(target: TargetConfig) -> Path:
    return Path(os.environ.get("SEL4_BUILD_DIR", str(target.default_sel4_build_dir)))


def image_name_from_env(target: TargetConfig) -> str:
    return os.environ.get("SEL4_IMAGE_NAME", target.image_name)


def image_suffix_from_env(target: TargetConfig) -> str:
    return image_name_from_env(target).removeprefix("sel4test-driver-")


def platform_from_env(target: TargetConfig) -> str:
    return os.environ.get("SEL4_PLATFORM", target.platform)


def sel4_arch_from_env(target: TargetConfig) -> str:
    return os.environ.get("SEL4_ARCH", target.sel4_arch)


def strip_from_env(target: TargetConfig) -> str:
    return os.environ.get("STRIP", target.strip)


def infer_toolprefix_for(target: TargetConfig, extra_prefixes: Sequence[str] = ()) -> str | None:
    prefixes = tuple(extra_prefixes) + target.xv6_toolprefixes
    for tool_prefix in prefixes:
        if command_exists(f"{tool_prefix}gcc"):
            return tool_prefix
    return None
