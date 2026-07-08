"""Target architecture configuration shared by repository tools."""

from __future__ import annotations

import os
from dataclasses import dataclass
from pathlib import Path
from typing import Sequence

from tool_common import ROOT_DIR, command_exists, die


@dataclass(frozen=True)
class Sel4ArchSourceStatus:
    tree_dir: Path
    kernel_arch_dirs: tuple[Path, ...]
    libsel4_dir: Path
    elfloader_src_dirs: tuple[Path, ...]
    elfloader_include_dirs: tuple[Path, ...]

    @staticmethod
    def _has_any(paths: tuple[Path, ...]) -> bool:
        return any(path.is_dir() for path in paths)

    @property
    def has_kernel_arch(self) -> bool:
        return self._has_any(self.kernel_arch_dirs)

    @property
    def has_libsel4_arch(self) -> bool:
        return self.libsel4_dir.is_dir()

    @property
    def has_elfloader_src(self) -> bool:
        return self._has_any(self.elfloader_src_dirs)

    @property
    def has_elfloader_include(self) -> bool:
        return self._has_any(self.elfloader_include_dirs)

    @property
    def is_ready(self) -> bool:
        return (
            self.has_kernel_arch
            and self.has_libsel4_arch
            and self.has_elfloader_src
            and self.has_elfloader_include
        )

    def _relative(self, path: Path) -> str:
        return str(path.relative_to(self.tree_dir))

    def _relative_any(self, paths: tuple[Path, ...]) -> str:
        return " or ".join(self._relative(path) for path in paths)

    def missing_descriptions(self) -> list[str]:
        missing: list[str] = []
        if not self.has_kernel_arch:
            missing.append(self._relative_any(self.kernel_arch_dirs))
        if not self.has_libsel4_arch:
            missing.append(self._relative(self.libsel4_dir))
        if not self.has_elfloader_src:
            missing.append(self._relative_any(self.elfloader_src_dirs))
        if not self.has_elfloader_include:
            missing.append(self._relative_any(self.elfloader_include_dirs))
        return missing


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
    qemu_bios: str | None
    xv6_dir_name: str
    xv6_toolprefixes: tuple[str, ...]
    xv6_march: str
    xv6_mabi: str
    xv6_disk_transport: str

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
                "-nic",
                "none",
            ]
        )
        if self.qemu_bios is not None:
            cmd.extend(["-bios", self.qemu_bios])
        return cmd

    def xv6_fs_device_args(self, fs_img: Path) -> list[str]:
        drive_args = [
            "-drive",
            f"file={fs_img},if=none,format=raw,id=xv6fs",
        ]
        if self.xv6_disk_transport == "virtio-mmio":
            return [
                "-global",
                "virtio-mmio.force-legacy=false",
                *drive_args,
                "-device",
                "virtio-blk-device,drive=xv6fs,bus=virtio-mmio-bus.0",
            ]
        if self.xv6_disk_transport == "virtio-pci":
            return [
                *drive_args,
                "-device",
                "virtio-blk-pci,drive=xv6fs,disable-legacy=on,disable-modern=off,addr=4",
            ]
        die("target", f"unsupported xv6 disk transport: {self.xv6_disk_transport}")

    def require_qemu(self, prefix: str) -> None:
        if not command_exists(self.qemu):
            die(prefix, f"{self.qemu} not on PATH; activate the flake dev shell")

    def sel4_arch_source_status(self, sel4_tree_dir: Path) -> Sel4ArchSourceStatus:
        arch_candidates = tuple(dict.fromkeys((self.sel4_source_arch, self.sel4_arch)))
        arch_dirs = tuple(
            sel4_tree_dir / "kernel" / "src" / "arch" / arch
            for arch in arch_candidates
        )
        libsel4_dir = (
            sel4_tree_dir
            / "kernel"
            / "libsel4"
            / "sel4_arch_include"
            / self.sel4_arch
        )
        elfloader_src_dirs = tuple(
            sel4_tree_dir / "tools" / "seL4" / "elfloader-tool" / "src" / f"arch-{arch}"
            for arch in arch_candidates
        )
        elfloader_include_dirs = tuple(
            sel4_tree_dir
            / "tools"
            / "seL4"
            / "elfloader-tool"
            / "include"
            / f"arch-{arch}"
            for arch in arch_candidates
        )
        return Sel4ArchSourceStatus(
            tree_dir=sel4_tree_dir,
            kernel_arch_dirs=arch_dirs,
            libsel4_dir=libsel4_dir,
            elfloader_src_dirs=elfloader_src_dirs,
            elfloader_include_dirs=elfloader_include_dirs,
        )

    def require_sel4_arch_source(self, prefix: str, sel4_tree_dir: Path) -> None:
        status = self.sel4_arch_source_status(sel4_tree_dir)
        if status.is_ready:
            return

        port_hint = (
            "Add a LoongArch seL4/libsel4/elfloader port"
            if self.name == "loongarch64"
            else f"Add an {self.name} seL4/libsel4/elfloader port"
        )
        die(
            prefix,
            (
                f"official sel4test for ARCH={self.name} is not available in {sel4_tree_dir}; "
                f"missing {', '.join(status.missing_descriptions())}. {port_hint} "
                "or set SEL4_TREE_DIR to one before packing."
            ),
        )


DEFAULT_SEL4_TREE_DIR = ROOT_DIR / "third_party" / "sel4test"


def sel4_tree_dir_from_env(build_dir: Path) -> Path:
    explicit = os.environ.get("SEL4_TREE_DIR") or os.environ.get("SEL4_ROOT")
    if explicit:
        return Path(explicit)
    if (build_dir.parent / "init-build.sh").is_file():
        return build_dir.parent
    return DEFAULT_SEL4_TREE_DIR


TARGETS: dict[str, TargetConfig] = {
    "riscv64": TargetConfig(
        name="riscv64",
        rust_target="riscv64gc-unknown-none-elf",
        sel4_arch="riscv64",
        sel4_source_arch="riscv",
        platform="qemu-riscv-virt",
        image_name="sel4test-driver-image-riscv64-qemu-riscv-virt",
        default_sel4_build_dir=DEFAULT_SEL4_TREE_DIR / "build-riscv64",
        strip="riscv64-none-elf-strip",
        qemu="qemu-system-riscv64",
        qemu_machine="virt",
        qemu_cpu="rv64",
        qemu_bios="none",
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
        xv6_disk_transport="virtio-mmio",
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
        qemu_bios=None,
        xv6_dir_name="xv6-loongarch64",
        xv6_toolprefixes=(
            "loongarch64-none-elf-",
            "loongarch64-unknown-none-",
            "loongarch64-unknown-linux-gnu-",
            "loongarch64-linux-gnu-",
        ),
        xv6_march="loongarch64",
        xv6_mabi="lp64d",
        xv6_disk_transport="virtio-pci",
    ),
    "x86_64": TargetConfig(
        name="x86_64",
        rust_target="x86_64-unknown-none",
        sel4_arch="x86_64",
        sel4_source_arch="x86",
        platform="pc99",
        image_name="sel4test-driver-image-x86_64-pc99",
        default_sel4_build_dir=DEFAULT_SEL4_TREE_DIR / "build-x86_64",
        strip="x86_64-elf-strip",
        qemu="qemu-system-x86_64",
        qemu_machine="pc",
        qemu_cpu=None,
        qemu_bios=None,
        xv6_dir_name="xv6-x86_64",
        xv6_toolprefixes=(
            "x86_64-none-elf-",
            "x86_64-unknown-none-",
            "x86_64-elf-",
            "x86_64-linux-gnu-",
            "x86_64-unknown-linux-gnu-",
        ),
        xv6_march="x86-64",
        xv6_mabi="",
        xv6_disk_transport="virtio-pci",
    ),
}


def normalize_arch(value: str) -> str:
    normalized = value.strip().lower().replace("_", "-")
    if normalized in ("", "riscv", "riscv64", "rv64"):
        return "riscv64"
    if normalized in ("loongarch", "loongarch64", "la64"):
        return "loongarch64"
    if normalized in ("x86-64", "x86_64", "amd64"):
        return "x86_64"
    return normalized


def arch_from_env() -> str:
    arch = os.environ.get("ARCH", "")
    if arch:
        return normalize_arch(arch)

    rust_target = os.environ.get("RUST_TARGET", "")
    if rust_target.startswith("loongarch64-"):
        return "loongarch64"
    if rust_target.startswith("x86_64-"):
        return "x86_64"
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


def require_supported_xv6_user_abi(prefix: str, target: TargetConfig, mabi: str) -> None:
    if target.name == "loongarch64" and mabi != target.xv6_mabi:
        die(
            prefix,
            (
                f"ARCH=loongarch64 currently requires XV6_USER_MABI={target.xv6_mabi}; "
                "the LoongArch xv6 compatibility stack is configured for double-float."
            ),
        )
