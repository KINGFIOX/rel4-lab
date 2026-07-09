"""Paths for the Rust kernel's seL4-style architecture split."""

from __future__ import annotations

from pathlib import Path

from tool_common import ROOT_DIR


def arch_dir(target_name: str) -> Path:
    return ROOT_DIR / "kernel" / "src" / "arch" / target_name


def arch_kernel(target_name: str, name: str) -> Path:
    return arch_dir(target_name) / "kernel" / name


def arch_machine(target_name: str, name: str) -> Path:
    return arch_dir(target_name) / "machine" / name


def arch_object(target_name: str, name: str) -> Path:
    return arch_dir(target_name) / "object" / name


def arch_smp(target_name: str, name: str) -> Path:
    return arch_dir(target_name) / "smp" / name


def arch_plat(target_name: str, name: str = "mod.rs") -> Path:
    return arch_dir(target_name) / "plat" / name


def trap_asm(target_name: str) -> Path:
    return arch_dir(target_name) / "trap.S"


def trap_rs(target_name: str) -> Path:
    return arch_kernel(target_name, "trap.rs")


def boot_rs(target_name: str) -> Path:
    return arch_kernel(target_name, "boot.rs")


def paging_rs(target_name: str) -> Path:
    return arch_machine(target_name, "paging.rs")


def csr_rs(target_name: str) -> Path:
    return arch_machine(target_name, "csr.rs")


def irq_rs(target_name: str) -> Path:
    return arch_machine(target_name, "irq.rs")


def vspace_rs(target_name: str) -> Path:
    return arch_object(target_name, "vspace.rs")


def ipi_rs(target_name: str) -> Path:
    return arch_smp(target_name, "ipi.rs")


def smp_mod_rs(target_name: str) -> Path:
    return arch_smp(target_name, "mod.rs")


def fpu_rs(target_name: str) -> Path:
    return arch_machine(target_name, "fpu.rs")
