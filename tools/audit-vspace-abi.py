#!/usr/bin/env python3
"""Audit architecture VSpace and page-table ABI constants."""

from __future__ import annotations

import argparse
import ast
import operator
import re
import sys
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parent))
from target_config import target_from_env
from tool_common import ROOT_DIR, log


PREFIX = "audit-vspace-abi"

CONST_RE = re.compile(
    r"(?:pub\s+)?const\s+([A-Z0-9_]+)\s*:\s*[^=]+=\s*(?P<expr>.*?);",
    re.S,
)
NUMBER_RE = re.compile(r"\b0x[0-9a-fA-F_]+|\b0b[01_]+|\b\d[\d_]*")

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
    expr = " ".join(line.strip() for line in expr.splitlines())
    return expr.strip()


def eval_expr(expr: str, symbols: dict[str, int]) -> int:
    tree = ast.parse(clean_expr(expr), mode="eval")

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
    symbols = dict(initial or {})
    for match in CONST_RE.finditer(path.read_text()):
        name = match.group(1)
        expr = match.group("expr").strip()
        if "{" in expr or "::" in expr:
            continue
        try:
            symbols[name] = eval_expr(expr, symbols)
        except (SyntaxError, ValueError):
            continue
    return symbols


def expect(errors: list[str], label: str, got: int | None, expected: int) -> None:
    if got is None:
        errors.append(f"{label} is missing")
    elif got != expected:
        errors.append(f"{label}=0x{got:x}, expected 0x{expected:x}")


def require_text(errors: list[str], path: Path, text: str, description: str) -> None:
    if text not in path.read_text():
        errors.append(f"{path.relative_to(ROOT_DIR)} is missing {description}: {text}")


def require_regex(errors: list[str], path: Path, pattern: str, description: str) -> None:
    if re.search(pattern, path.read_text(), re.S) is None:
        errors.append(f"{path.relative_to(ROOT_DIR)} is missing {description}")


def audit_kva_to_pa_helpers(
    errors: list[str],
    abi_consts: dict[str, int],
    paths: tuple[Path, ...],
    target_name: str,
) -> None:
    phys_base = abi_consts.get("PHYS_BASE_RAW")
    kernel_elf_base = abi_consts.get("KERNEL_ELF_BASE")
    paddr_base = abi_consts.get("PADDR_BASE")
    pptr_base = abi_consts.get("PPTR_BASE")
    if None in (phys_base, kernel_elf_base, paddr_base, pptr_base):
        errors.append(f"{target_name} KVA/PA audit could not resolve direct-map constants")
        return
    if kernel_elf_base - phys_base != pptr_base - paddr_base:
        errors.append(
            f"{target_name} KERNEL_ELF_BASE-PHYS_BASE_RAW offset differs from "
            "PPTR_BASE-PADDR_BASE; kva_to_pa branch order would be ambiguous"
        )

    for path in paths:
        require_text(
            errors,
            path,
            "kva - (KERNEL_ELF_BASE as u64) + (PHYS_BASE_RAW as u64)",
            "kernel-ELF KVA to PA formula",
        )
        require_text(
            errors,
            path,
            "kva - (PPTR_BASE as u64) + (PADDR_BASE as u64)",
            "PSpace KVA to PA formula",
        )
        require_regex(
            errors,
            path,
            r"fn\s+kva_to_pa\([^)]*\)\s*->\s*u64\s*\{.*?if\s+kva\s*>=\s*"
            r"\(KERNEL_ELF_BASE\s+as\s+u64\)",
            "kernel-ELF-first kva_to_pa helper",
        )


def audit_common_paging(errors: list[str], consts: dict[str, int], target_name: str) -> None:
    expect(errors, f"{target_name} PAGE_SHIFT", consts.get("PAGE_SHIFT"), 12)
    expect(errors, f"{target_name} PAGE_SIZE", consts.get("PAGE_SIZE"), 0x1000)
    expect(errors, f"{target_name} LEAF_LEVEL", consts.get("LEAF_LEVEL"), 0)
    expect(errors, f"{target_name} ROOT_LEVEL", consts.get("ROOT_LEVEL"), 2)
    expect(
        errors,
        f"{target_name} ROOT_CHILD_COVERAGE_BITS",
        consts.get("ROOT_CHILD_COVERAGE_BITS"),
        30,
    )
    expect(
        errors,
        f"{target_name} LEAF_PARENT_COVERAGE_BITS",
        consts.get("LEAF_PARENT_COVERAGE_BITS"),
        21,
    )


def audit_loongarch64(errors: list[str]) -> None:
    abi_consts = parse_consts(ROOT_DIR / "kernel" / "src" / "abi" / "constants.rs")
    paging_rs = ROOT_DIR / "kernel" / "src" / "arch" / "loongarch64" / "paging.rs"
    csr_rs = ROOT_DIR / "kernel" / "src" / "arch" / "loongarch64" / "csr.rs"
    vspace_rs = ROOT_DIR / "kernel" / "src" / "arch" / "loongarch64" / "vspace.rs"
    asid_rs = ROOT_DIR / "kernel" / "src" / "object" / "asid.rs"
    boot_rs = ROOT_DIR / "kernel" / "src" / "kernel" / "boot.rs"
    invocation_rs = ROOT_DIR / "kernel" / "src" / "api" / "invocation.rs"
    paging = parse_consts(paging_rs, abi_consts)
    csr = parse_consts(csr_rs)
    vspace = parse_consts(vspace_rs, {**abi_consts, **paging})
    all_consts = {**abi_consts, **paging, **csr, **vspace}

    audit_common_paging(errors, paging, "loongarch64")
    for name, value in (
        ("PTE_V", 1 << 0),
        ("PTE_D", 1 << 1),
        ("PTE_PLV_SHIFT", 2),
        ("PTE_PLV_KERNEL", 0),
        ("PTE_PLV_USER", 0b11 << 2),
        ("PTE_MAT_SHIFT", 4),
        ("PTE_MAT_SUC", 0),
        ("PTE_MAT_CC", 0b01 << 4),
        ("PTE_G", 1 << 6),
        ("PTE_PRESENT", 1 << 7),
        ("PTE_W", 1 << 8),
        ("PTE_PFN_SHIFT", 12),
        ("PTE_NR", 1 << 61),
        ("PTE_NX", 1 << 62),
        ("PTE_RPLV", 1 << 63),
    ):
        expect(errors, f"loongarch64 {name}", paging.get(name), value)

    expect(errors, "loongarch64 ASID_MASK", csr.get("ASID_MASK"), 0x3ff)
    require_regex(
        errors,
        asid_rs,
        r"#\[cfg\(target_arch\s*=\s*\"loongarch64\"\)\]\s*"
        r"const\s+ARCH_ASID_BITS\s*:\s*usize\s*=\s*10\s*;",
        "LoongArch hardware ASID width",
    )
    require_text(
        errors,
        asid_rs,
        "const ASID_TABLE_LEN: usize = 1 << ARCH_ASID_BITS;",
        "ASID table sized by architecture ASID width",
    )
    require_text(
        errors,
        asid_rs,
        "const ASID_POOL_COUNT: usize = ASID_TABLE_LEN / ASID_POOL_ENTRY_COUNT;",
        "ASID pool count derived from architecture ASID width",
    )
    require_regex(
        errors,
        asid_rs,
        r"pub\s+fn\s+delete_pool\([^)]*\)\s*\{.*?if\s+deleted\s*\{\s*"
        r"crate::kernel::smp::sfence_vma_all_harts\(\);",
        "ASID pool deletion full TLB shootdown",
    )
    for name, value in (
        ("CSR_ASID", 0x018),
        ("CSR_PGDL", 0x019),
        ("CSR_PWCL", 0x01C),
        ("CSR_PWCH", 0x01D),
        ("CSR_STLBPS", 0x01E),
        ("CSR_DMW0", 0x180),
        ("CSR_DMW1", 0x181),
    ):
        expect(errors, f"loongarch64 {name}", csr.get(name), value)

    expect(errors, "loongarch64 USER_ROOT_ENTRIES", vspace.get("USER_ROOT_ENTRIES"), 256)
    expect(errors, "loongarch64 USER_TOP", vspace.get("USER_TOP"), 0x4000_0000_00)
    expect(errors, "loongarch64 PHYS_BASE_RAW", abi_consts.get("PHYS_BASE_RAW"), 0x0020_0000)
    expect(errors, "loongarch64 PADDR_BASE", abi_consts.get("PADDR_BASE"), 0)
    expect(errors, "loongarch64 PPTR_BASE", abi_consts.get("PPTR_BASE"), 0)
    expect(errors, "loongarch64 PPTR_TOP", abi_consts.get("PPTR_TOP"), 0x0000_0002_0000_0000)
    expect(
        errors,
        "loongarch64 KERNEL_ELF_BASE",
        abi_consts.get("KERNEL_ELF_BASE"),
        0x0020_0000,
    )
    expect(errors, "loongarch64 PAGE_WALK_DIR1_BASE", vspace.get("PAGE_WALK_DIR1_BASE"), 21)
    expect(errors, "loongarch64 PAGE_WALK_DIR2_BASE", vspace.get("PAGE_WALK_DIR2_BASE"), 30)
    expect(
        errors,
        "loongarch64 PAGE_WALK_CONTROL_LOW",
        vspace.get("PAGE_WALK_CONTROL_LOW"),
        0x13e4_d52c,
    )
    expect(errors, "loongarch64 DMW_MMIO_VSEG", vspace.get("DMW_MMIO_VSEG"), 0x8)
    expect(
        errors,
        "loongarch64 DMW_MMIO_ALIAS_BASE",
        vspace.get("DMW_MMIO_ALIAS_BASE"),
        0x8000_0000_0000_0000,
    )
    expect(errors, "loongarch64 DMW_LOW_DIRECT", vspace.get("DMW_LOW_DIRECT"), 0x11)
    expect(
        errors,
        "loongarch64 DMW_MMIO_DIRECT",
        vspace.get("DMW_MMIO_DIRECT"),
        0x8000_0000_0000_0001,
    )

    require_text(
        errors,
        vspace_rs,
        "csr::set_pgdl((satp_val & !0xfffu64) as usize)",
        "PGDL root mask",
    )
    require_text(
        errors,
        vspace_rs,
        "csr::set_asid((satp_val & csr::ASID_MASK as u64) as usize)",
        "ASID mask on switch",
    )
    require_text(errors, vspace_rs, "csr::set_stlbps(PAGE_SHIFT)", "STLB page-size setup")
    require_text(errors, vspace_rs, "csr::set_dmw0(DMW_LOW_DIRECT)", "low direct-map setup")
    require_text(errors, vspace_rs, "csr::set_dmw1(DMW_MMIO_DIRECT)", "MMIO direct-map setup")
    require_text(errors, vspace_rs, "csr::set_dmw2(0)", "DMW2 disabled")
    require_text(errors, vspace_rs, "csr::set_dmw3(0)", "DMW3 disabled")
    require_text(errors, vspace_rs, "PTE_MAT_SUC", "device frame uncached MAT")
    require_text(errors, vspace_rs, "PTE_MAT_CC", "normal frame cached MAT")
    require_regex(
        errors,
        vspace_rs,
        r"fn\s+copy_kernel_mappings_to\([^)]*\)\s*\{[^}]*LoongArch keeps kernel access",
        "no-op kernel mapping copy rationale",
    )
    require_regex(
        errors,
        boot_rs,
        r"let\s+satp\s*=\s*satp_for\(root_pt,\s*ROOTSERVER_ASID\s+as\s+u64\);.*?"
        r"crate::kernel::smp::publish_kernel_satp\(satp\);.*?"
        r"unsafe\s*\{\s*switch_satp\(satp\)\s*\};.*?"
        r"crate::machine::console::init\(\);.*?"
        r"crate::arch::current::irq::init\(\);",
        "LoongArch boot configures DMW/MMIO access before console and IRQ init",
    )
    audit_kva_to_pa_helpers(errors, abi_consts, (boot_rs, invocation_rs), "loongarch64")

    # Ensure the parsed symbol graph includes all imported constants used above.
    for name in ("PT_INDEX_BITS", "PADDR_BASE", "PPTR_BASE", "PPTR_TOP", "KERNEL_ELF_BASE"):
        if name not in all_consts:
            errors.append(f"loongarch64 VSpace audit could not resolve {name}")


def audit_riscv64(errors: list[str]) -> None:
    abi_consts = parse_consts(ROOT_DIR / "kernel" / "src" / "abi" / "constants.rs")
    sv39_rs = ROOT_DIR / "kernel" / "src" / "arch" / "riscv64" / "sv39.rs"
    sv39 = parse_consts(sv39_rs, abi_consts)
    audit_common_paging(errors, sv39, "riscv64")
    for name, value in (
        ("PTE_V", 1 << 0),
        ("PTE_R", 1 << 1),
        ("PTE_W", 1 << 2),
        ("PTE_X", 1 << 3),
        ("PTE_U", 1 << 4),
        ("PTE_G", 1 << 5),
        ("PTE_A", 1 << 6),
        ("PTE_D", 1 << 7),
    ):
        expect(errors, f"riscv64 {name}", sv39.get(name), value)
    require_text(errors, sv39_rs, "(8u64 << 60)", "Sv39 satp mode")
    require_text(errors, sv39_rs, "((asid & 0xFFFF) << 44)", "Sv39 ASID field")
    require_text(
        errors,
        sv39_rs,
        "((root_pt_paddr >> RISCV_PG_SHIFT) & ((1u64 << 44) - 1))",
        "Sv39 PPN field",
    )


def main(argv: list[str]) -> int:
    parser = argparse.ArgumentParser(description="Check architecture VSpace ABI constants.")
    parser.parse_args(argv)

    target = target_from_env(PREFIX)
    errors: list[str] = []
    if target.name == "loongarch64":
        audit_loongarch64(errors)
    elif target.name == "riscv64":
        audit_riscv64(errors)
    else:
        errors.append(f"unsupported target {target.name}")

    if errors:
        for error in errors:
            log(PREFIX, f"FAIL: {error}")
        return 1

    print(f"PASS: {target.name} VSpace ABI constants")
    return 0


if __name__ == "__main__":
    raise SystemExit(main(sys.argv[1:]))
