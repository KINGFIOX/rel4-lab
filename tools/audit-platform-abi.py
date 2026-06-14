#!/usr/bin/env python3
"""Audit kernel/userspace platform MMIO ABI constants."""

from __future__ import annotations

import argparse
import ast
import operator
import re
import sys
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parent))
from target_config import target_from_env
from tool_common import ROOT_DIR, die, log


PREFIX = "audit-platform-abi"
PAGE_SIZE = 0x1000

CONST_RE = re.compile(
    r"(?:pub\s+)?const\s+([A-Z0-9_]+)\s*:\s*[^=]+=\s*(?P<expr>.*?);",
    re.S,
)
NUMBER_RE = re.compile(r"\b0x[0-9a-fA-F_]+|\b\d[\d_]*")
REGION_RE = re.compile(
    r"pub\s+const\s+DEVICE_UNTYPED_REGIONS\s*:\s*&\[\(u64,\s*u64\)\]\s*=\s*&\[(?P<body>.*?)\];",
    re.S,
)
TUPLE_RE = re.compile(r"\(\s*([^,]+?)\s*,\s*([^)]+?)\s*\)")

BIN_OPS = {
    ast.Add: operator.add,
    ast.Sub: operator.sub,
    ast.Mult: operator.mul,
    ast.Div: operator.floordiv,
    ast.FloorDiv: operator.floordiv,
    ast.LShift: operator.lshift,
    ast.RShift: operator.rshift,
    ast.BitOr: operator.or_,
    ast.BitAnd: operator.and_,
}


def strip_comments(text: str) -> str:
    return re.sub(r"//.*", "", text)


def clean_expr(expr: str) -> str:
    expr = strip_comments(expr)
    expr = re.sub(r"\bas\s+(?:u8|u16|u32|u64|usize|i32|i64|isize)\b", "", expr)
    expr = NUMBER_RE.sub(lambda match: match.group(0).replace("_", ""), expr)
    return expr.strip()


def eval_expr(expr: str, symbols: dict[str, int]) -> int:
    cleaned = clean_expr(expr)
    tree = ast.parse(cleaned, mode="eval")

    def visit(node: ast.AST) -> int:
        if isinstance(node, ast.Expression):
            return visit(node.body)
        if isinstance(node, ast.Constant) and isinstance(node.value, int):
            return node.value
        if isinstance(node, ast.Name):
            if node.id not in symbols:
                raise ValueError(f"unknown symbol {node.id}")
            return symbols[node.id]
        if isinstance(node, ast.UnaryOp) and isinstance(node.op, ast.USub):
            return -visit(node.operand)
        if isinstance(node, ast.BinOp):
            op = BIN_OPS.get(type(node.op))
            if op is None:
                raise ValueError(f"unsupported operator {type(node.op).__name__}")
            return op(visit(node.left), visit(node.right))
        raise ValueError(f"unsupported expression node {type(node).__name__}")

    return visit(tree)


def parse_consts(path: Path, initial: dict[str, int] | None = None) -> dict[str, int]:
    symbols: dict[str, int] = dict(initial or {})
    text = path.read_text()
    for match in CONST_RE.finditer(text):
        name = match.group(1)
        expr = match.group("expr").strip()
        if "{" in expr or "::" in expr:
            continue
        try:
            symbols[name] = eval_expr(expr, symbols)
        except (SyntaxError, ValueError):
            continue
    return symbols


def parse_device_regions(path: Path, symbols: dict[str, int]) -> list[tuple[int, int]]:
    text = path.read_text()
    match = REGION_RE.search(text)
    if not match:
        die(PREFIX, f"DEVICE_UNTYPED_REGIONS not found in {path}")
    regions: list[tuple[int, int]] = []
    body = strip_comments(match.group("body"))
    for start_expr, end_expr in TUPLE_RE.findall(body):
        regions.append((eval_expr(start_expr, symbols), eval_expr(end_expr, symbols)))
    if not regions:
        die(PREFIX, f"DEVICE_UNTYPED_REGIONS is empty in {path}")
    return regions


def covered_by(regions: list[tuple[int, int]], start: int, size: int) -> bool:
    end = start + size
    return any(start >= region_start and end <= region_end for region_start, region_end in regions)


def require_symbol(symbols: dict[str, int], name: str, errors: list[str], context: str) -> int:
    value = symbols.get(name)
    if value is None:
        errors.append(f"{context}: missing {name}")
        return 0
    return value


def expect_equal(errors: list[str], label: str, got: int, expected: int) -> None:
    if got != expected:
        errors.append(f"{label}=0x{got:x}, expected 0x{expected:x}")


def expect_page_aligned(errors: list[str], label: str, value: int) -> None:
    if value % PAGE_SIZE != 0:
        errors.append(f"{label}=0x{value:x} is not {PAGE_SIZE:#x}-aligned")


def expect_covered(
    errors: list[str],
    label: str,
    regions: list[tuple[int, int]],
    start: int,
    size: int,
) -> None:
    if not covered_by(regions, start, size):
        region_text = ", ".join(f"[0x{lo:x}, 0x{hi:x})" for lo, hi in regions)
        errors.append(f"{label} [0x{start:x}, 0x{start + size:x}) is not covered by {region_text}")


def audit_common_device_window(
    errors: list[str],
    target_name: str,
    kernel_consts: dict[str, int],
    platform_consts: dict[str, int],
    regions: list[tuple[int, int]],
) -> None:
    device_base = require_symbol(platform_consts, "XV6_DEVICE_MMIO_BASE", errors, target_name)
    device_size = require_symbol(platform_consts, "XV6_DEVICE_MMIO_SIZE", errors, target_name)
    uart_frame = require_symbol(platform_consts, "UART0_MMIO_FRAME_BASE", errors, target_name)
    expect_equal(errors, "XV6_DEVICE_MMIO_BASE", device_base, uart_frame)
    expect_page_aligned(errors, "XV6_DEVICE_MMIO_BASE", device_base)
    expect_covered(errors, "XV6_DEVICE_MMIO window", regions, device_base, device_size)

    if "UART0_MMIO_BASE_PA" in kernel_consts:
        expect_equal(
            errors,
            "UART0_MMIO_BASE",
            require_symbol(platform_consts, "UART0_MMIO_BASE", errors, target_name),
            kernel_consts["UART0_MMIO_BASE_PA"],
        )
    if "UART0_MMIO_SIZE" in kernel_consts:
        expect_equal(
            errors,
            "UART0_MMIO_SIZE",
            require_symbol(platform_consts, "UART0_MMIO_SIZE", errors, target_name),
            kernel_consts["UART0_MMIO_SIZE"],
        )


def audit_loongarch64(
    kernel_consts: dict[str, int],
    platform_consts: dict[str, int],
    pci_consts: dict[str, int],
    irq_consts: dict[str, int],
    regions: list[tuple[int, int]],
) -> list[str]:
    errors: list[str] = []
    audit_common_device_window(errors, "loongarch64", kernel_consts, platform_consts, regions)

    io_base = require_symbol(kernel_consts, "PCI_IO_BASE_PA", errors, "kernel")
    io_port = require_symbol(kernel_consts, "PCI_DEBUG_UART_PORT", errors, "kernel")
    io_size = require_symbol(kernel_consts, "PCI_IO_SIZE", errors, "kernel")
    userspace_io_base = require_symbol(
        platform_consts, "LOONGARCH64_PCIE_IO_BASE", errors, "userspace"
    )
    userspace_io_size = require_symbol(
        platform_consts, "LOONGARCH64_PCIE_IO_SIZE", errors, "userspace"
    )
    virtio_io_port_base = require_symbol(
        pci_consts, "LOONGARCH64_PCIE_IO_PORT_BASE", errors, "virtio-disk-server"
    )
    expect_equal(errors, "LOONGARCH64_PCIE_IO_BASE", userspace_io_base, io_base + io_port)
    expect_equal(errors, "LOONGARCH64_PCIE_IO_PORT_BASE", virtio_io_port_base, io_port)
    expect_equal(errors, "LOONGARCH64_PCIE_IO_SIZE", userspace_io_size, io_size)

    for kernel_name, userspace_name in (
        ("PCI_ECAM_BASE_PA", "LOONGARCH64_PCIE_ECAM_BASE"),
        ("PCI_MEM_BASE_PA", "LOONGARCH64_PCIE_MEM_BASE"),
        ("PCI_MEM_SIZE", "LOONGARCH64_PCIE_MEM_SIZE"),
        ("PCH_MSI_BASE_PA", "LOONGARCH64_PCH_MSI_BASE"),
    ):
        expect_equal(
            errors,
            userspace_name,
            require_symbol(platform_consts, userspace_name, errors, "userspace"),
            require_symbol(kernel_consts, kernel_name, errors, "kernel"),
        )

    mapped_windows = (
        (
            "UART frame",
            require_symbol(platform_consts, "UART0_MMIO_FRAME_BASE", errors, "userspace"),
            PAGE_SIZE,
        ),
        (
            "PCI ECAM map",
            require_symbol(platform_consts, "LOONGARCH64_PCIE_ECAM_BASE", errors, "userspace"),
            require_symbol(platform_consts, "XV6_PCIE_ECAM_MAP_SIZE", errors, "userspace"),
        ),
        (
            "PCI I/O map",
            userspace_io_base,
            require_symbol(platform_consts, "XV6_PCIE_IO_MAP_SIZE", errors, "userspace"),
        ),
        (
            "PCI MEM map",
            require_symbol(platform_consts, "LOONGARCH64_PCIE_MEM_BASE", errors, "userspace"),
            require_symbol(platform_consts, "XV6_PCIE_MEM_MAP_SIZE", errors, "userspace"),
        ),
        (
            "PCH MSI map",
            require_symbol(platform_consts, "LOONGARCH64_PCH_MSI_BASE", errors, "userspace"),
            require_symbol(platform_consts, "XV6_PCIE_MSI_MAP_SIZE", errors, "userspace"),
        ),
    )
    for label, start, size in mapped_windows:
        expect_page_aligned(errors, label, start)
        expect_page_aligned(errors, f"{label} size", size)
        expect_covered(errors, label, regions, start, size)

    expect_equal(
        errors,
        "XV6_PCIE_IO_MAP_SIZE",
        require_symbol(platform_consts, "XV6_PCIE_IO_MAP_SIZE", errors, "userspace"),
        userspace_io_size,
    )
    if require_symbol(platform_consts, "XV6_PCIE_ECAM_MAP_SIZE", errors, "userspace") > require_symbol(
        platform_consts, "LOONGARCH64_PCIE_ECAM_SIZE", errors, "userspace"
    ):
        errors.append("XV6_PCIE_ECAM_MAP_SIZE exceeds LOONGARCH64_PCIE_ECAM_SIZE")
    if require_symbol(platform_consts, "XV6_PCIE_MEM_MAP_SIZE", errors, "userspace") > require_symbol(
        platform_consts, "LOONGARCH64_PCIE_MEM_SIZE", errors, "userspace"
    ):
        errors.append("XV6_PCIE_MEM_MAP_SIZE exceeds LOONGARCH64_PCIE_MEM_SIZE")

    legacy_irq_base = require_symbol(
        platform_consts, "LOONGARCH64_PCIE_LEGACY_IRQ_BASE", errors, "userspace"
    )
    legacy_irq_count = require_symbol(
        platform_consts, "LOONGARCH64_PCIE_LEGACY_IRQ_COUNT", errors, "userspace"
    )
    extioi_irqs = require_symbol(irq_consts, "EXTIOI_IRQS", errors, "kernel")
    if legacy_irq_base == 0 or legacy_irq_base + legacy_irq_count > extioi_irqs:
        errors.append(
            "LOONGARCH64_PCIE_LEGACY_IRQ range "
            f"[{legacy_irq_base}, {legacy_irq_base + legacy_irq_count}) exceeds EXTIOI_IRQS={extioi_irqs}"
        )
    msi_base_vector = require_symbol(
        platform_consts, "LOONGARCH64_PCH_MSI_BASE_VECTOR", errors, "userspace"
    )
    msi_vectors = require_symbol(
        platform_consts, "LOONGARCH64_PCH_MSI_NUM_VECTORS", errors, "userspace"
    )
    if msi_base_vector == 0 or msi_base_vector + msi_vectors > extioi_irqs:
        errors.append(
            "LOONGARCH64_PCH_MSI vector range "
            f"[{msi_base_vector}, {msi_base_vector + msi_vectors}) exceeds EXTIOI_IRQS={extioi_irqs}"
        )

    return errors


def audit_riscv64(
    kernel_consts: dict[str, int],
    platform_consts: dict[str, int],
    regions: list[tuple[int, int]],
) -> list[str]:
    errors: list[str] = []
    audit_common_device_window(errors, "riscv64", kernel_consts, platform_consts, regions)

    mapped_windows = (
        (
            "UART frame",
            require_symbol(platform_consts, "UART0_MMIO_FRAME_BASE", errors, "userspace"),
            require_symbol(platform_consts, "UART0_MMIO_SIZE", errors, "userspace"),
        ),
        (
            "VirtIO MMIO frame",
            require_symbol(platform_consts, "VIRTIO_MMIO_FRAME_BASE", errors, "userspace"),
            require_symbol(platform_consts, "VIRTIO_MMIO_SIZE", errors, "userspace"),
        ),
    )
    for label, start, size in mapped_windows:
        expect_page_aligned(errors, label, start)
        expect_page_aligned(errors, f"{label} size", size)
        expect_covered(errors, label, regions, start, size)

    return errors


def main(argv: list[str]) -> int:
    parser = argparse.ArgumentParser(
        description="Check kernel and xv6 platform MMIO ABI constants."
    )
    parser.parse_args(argv)

    target = target_from_env(PREFIX)
    shared_platform_rs = ROOT_DIR / "userspace" / "xv6-abi" / "src" / "platform" / "mod.rs"
    kernel_platform_rs = ROOT_DIR / "kernel" / "src" / "arch" / target.name / "platform.rs"
    userspace_platform_rs = (
        ROOT_DIR / "userspace" / "xv6-abi" / "src" / "platform" / f"{target.name}.rs"
    )
    if not kernel_platform_rs.is_file():
        die(PREFIX, f"kernel platform source not found: {kernel_platform_rs}")
    if not userspace_platform_rs.is_file():
        die(PREFIX, f"xv6 platform source not found: {userspace_platform_rs}")

    shared_consts = parse_consts(shared_platform_rs)
    kernel_consts = parse_consts(kernel_platform_rs)
    platform_consts = parse_consts(userspace_platform_rs, shared_consts)
    regions = parse_device_regions(kernel_platform_rs, kernel_consts)

    if target.name == "loongarch64":
        pci_consts = parse_consts(
            ROOT_DIR / "userspace" / "virtio-disk-server" / "src" / "device" / "pci.rs"
        )
        irq_consts = parse_consts(ROOT_DIR / "kernel" / "src" / "machine" / "loongarch_irq.rs")
        errors = audit_loongarch64(kernel_consts, platform_consts, pci_consts, irq_consts, regions)
    elif target.name == "riscv64":
        errors = audit_riscv64(kernel_consts, platform_consts, regions)
    else:
        die(PREFIX, f"unsupported target {target.name}")

    if errors:
        for error in errors:
            log(PREFIX, f"FAIL: {error}")
        return 1

    print(f"PASS: {target.name} platform ABI constants")
    return 0


if __name__ == "__main__":
    raise SystemExit(main(sys.argv[1:]))
