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


def parse_c_defines(path: Path) -> dict[str, int]:
    symbols: dict[str, int] = {}
    define_re = re.compile(r"^\s*#define\s+([A-Za-z0-9_]+)\s+(.+?)\s*$")
    for line in path.read_text().splitlines():
        match = define_re.match(line)
        if match is None:
            continue
        name = match.group(1)
        expr = match.group(2).split("/*", 1)[0].strip()
        if not expr:
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
    trap_rs = ROOT_DIR / "kernel" / "src" / "arch" / "loongarch64" / "trap.rs"
    asid_rs = ROOT_DIR / "kernel" / "src" / "object" / "asid.rs"
    boot_rs = ROOT_DIR / "kernel" / "src" / "kernel" / "boot.rs"
    invocation_rs = ROOT_DIR / "kernel" / "src" / "api" / "invocation.rs"
    libsel4_consts_h = (
        ROOT_DIR
        / "third_party"
        / "sel4test"
        / "kernel"
        / "libsel4"
        / "sel4_arch_include"
        / "loongarch64"
        / "sel4"
        / "sel4_arch"
        / "constants.h"
    )
    paging = parse_consts(paging_rs, abi_consts)
    csr = parse_consts(csr_rs)
    vspace = parse_consts(vspace_rs, {**abi_consts, **paging})
    libsel4 = parse_c_defines(libsel4_consts_h)
    all_consts = {**abi_consts, **paging, **csr, **vspace}

    audit_common_paging(errors, paging, "loongarch64")
    for name, value in (
        ("PTE_V", 1 << 0),
        ("PTE_D", 1 << 1),
        ("PTE_PLV_SHIFT", 2),
        ("PTE_PLV_MASK", 0b11 << 2),
        ("PTE_PLV_KERNEL", 0),
        ("PTE_PLV_USER", 0b11 << 2),
        ("PTE_MAT_SHIFT", 4),
        ("PTE_MAT_SUC", 0),
        ("PTE_MAT_CC", 0b01 << 4),
        ("PTE_MAT_WUC", 0b10 << 4),
        ("PTE_G", 1 << 6),
        ("PTE_HUGE", 1 << 6),
        ("PTE_PRESENT", 1 << 7),
        ("PTE_W", 1 << 8),
        ("PTE_MODIFIED", 1 << 9),
        ("PTE_SPECIAL", 1 << 11),
        ("PTE_PFN_SHIFT", 12),
        ("PTE_PFN_MASK", (1 << 36) - 1),
        ("PTE_NR", 1 << 61),
        ("PTE_NX", 1 << 62),
        ("PTE_RPLV", 1 << 63),
        ("PTE_KERNEL_RWX", (1 << 7) | (1 << 0) | (1 << 1) | (1 << 8) | (1 << 6) | (0b01 << 4)),
        (
            "PTE_USER_RW",
            (1 << 7)
            | (1 << 0)
            | (1 << 1)
            | (1 << 8)
            | (0b11 << 2)
            | (0b01 << 4)
            | (1 << 62)
            | (1 << 63),
        ),
        ("PTE_USER_RX", (1 << 7) | (1 << 0) | (0b11 << 2) | (0b01 << 4) | (1 << 63)),
        (
            "PTE_USER_RWX",
            (1 << 7) | (1 << 0) | (1 << 1) | (1 << 8) | (0b11 << 2) | (0b01 << 4) | (1 << 63),
        ),
    ):
        expect(errors, f"loongarch64 {name}", paging.get(name), value)
    require_regex(
        errors,
        paging_rs,
        r"pub\s+const\s+fn\s+ppn\(self\)\s*->\s*u64\s*\{\s*"
        r"\(self\.0\s*>>\s*PTE_PFN_SHIFT\)\s*&\s*PTE_PFN_MASK\s*"
        r"\}",
        "LoongArch PTE PFN decode uses PTE_PFN_MASK",
    )
    require_regex(
        errors,
        paging_rs,
        r"pub\s+const\s+fn\s+next\(pt_paddr:\s*u64\)\s*->\s*Pte\s*\{\s*"
        r"Pte\(pt_paddr\s*&\s*!\(\(PAGE_SIZE\s+as\s+u64\)\s*-\s*1\)\)\s*"
        r"\}",
        "LoongArch non-leaf PTEs are pure page-table physical addresses",
    )
    require_regex(
        errors,
        paging_rs,
        r"pub\s+const\s+fn\s+is_valid\(self\)\s*->\s*bool\s*\{\s*"
        r"self\.0\s*!=\s*0\s*"
        r"\}",
        "LoongArch software page-table validity accepts pure-address non-leaf entries",
    )

    expect(errors, "loongarch64 ASID_MASK", csr.get("ASID_MASK"), 0x3ff)
    expect(errors, "loongarch64 libsel4 seL4_NumASIDPoolsBits", libsel4.get("seL4_NumASIDPoolsBits"), 1)
    expect(
        errors,
        "loongarch64 libsel4 seL4_ASIDPoolIndexBits",
        libsel4.get("seL4_ASIDPoolIndexBits"),
        9,
    )
    if (
        libsel4.get("seL4_NumASIDPoolsBits") is not None
        and libsel4.get("seL4_ASIDPoolIndexBits") is not None
    ):
        expect(
            errors,
            "loongarch64 libsel4 ASID bits",
            libsel4["seL4_NumASIDPoolsBits"] + libsel4["seL4_ASIDPoolIndexBits"],
            10,
        )
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
        ("CSR_CRMD", 0x000),
        ("CSR_ASID", 0x018),
        ("CSR_PGDL", 0x019),
        ("CSR_PGDH", 0x01A),
        ("CSR_PGD", 0x01B),
        ("CSR_PWCL", 0x01C),
        ("CSR_PWCH", 0x01D),
        ("CSR_STLBPS", 0x01E),
        ("CSR_TLBRENTRY", 0x088),
        ("CSR_DMW0", 0x180),
        ("CSR_DMW1", 0x181),
        ("CSR_DMW2", 0x182),
        ("CSR_DMW3", 0x183),
        ("INVTLB_ALL", 0x00),
        ("INVTLB_ASID", 0x04),
        ("INVTLB_ADDR_G_OR_ASID", 0x06),
    ):
        expect(errors, f"loongarch64 {name}", csr.get(name), value)

    require_regex(
        errors,
        csr_rs,
        r"pub\s+fn\s+dbar\(\)\s*\{\s*unsafe\s*\{\s*asm!\(\"dbar 0\",\s*options\(nostack\)\)\s*\};\s*\}",
        "LoongArch dbar compiler-visible memory barrier",
    )
    require_regex(
        errors,
        csr_rs,
        r"pub\s+fn\s+ibar\(\)\s*\{\s*unsafe\s*\{\s*asm!\(\"ibar 0\",\s*options\(nostack\)\)\s*\};\s*\}",
        "LoongArch ibar compiler-visible instruction barrier",
    )
    require_regex(
        errors,
        csr_rs,
        r"pub\s+fn\s+sfence_vma_all\(\)\s*\{.*?"
        r"dbar\(\);.*?"
        r"asm!\(\"invtlb \{op\}, \$zero, \$zero\",\s*op\s*=\s*const\s*INVTLB_ALL,.*?"
        r"dbar\(\);",
        "LoongArch full TLB flush uses INVTLB_ALL with pre/post barriers",
    )
    require_regex(
        errors,
        csr_rs,
        r"pub\s+fn\s+sfence_vma_va\(vaddr:\s*usize\)\s*\{.*?"
        r"let\s+asid\s*=\s*asid\(\)\s*&\s*ASID_MASK;.*?"
        r"dbar\(\);.*?"
        r"op\s*=\s*const\s*INVTLB_ADDR_G_OR_ASID,.*?"
        r"asid\s*=\s*in\(reg\)\s*asid,.*?"
        r"vaddr\s*=\s*in\(reg\)\s*vaddr,.*?"
        r"dbar\(\);",
        "LoongArch VA TLB flush uses current ASID with pre/post barriers",
    )
    require_regex(
        errors,
        csr_rs,
        r"pub\s+fn\s+sfence_vma_asid\(asid:\s*usize\)\s*\{.*?"
        r"dbar\(\);.*?"
        r"op\s*=\s*const\s*INVTLB_ASID,.*?"
        r"asid\s*=\s*in\(reg\)\s*asid\s*&\s*ASID_MASK,.*?"
        r"dbar\(\);",
        "LoongArch ASID TLB flush masks ASID with pre/post barriers",
    )

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
    expect(errors, "loongarch64 PAGE_WALK_DIR0_BASE", vspace.get("PAGE_WALK_DIR0_BASE"), 21)
    expect(errors, "loongarch64 PAGE_WALK_DIR2_BASE", vspace.get("PAGE_WALK_DIR2_BASE"), 30)
    expect(
        errors,
        "loongarch64 PAGE_WALK_CONTROL_LOW",
        vspace.get("PAGE_WALK_CONTROL_LOW"),
        0x4_d52c,
    )
    expect(
        errors,
        "loongarch64 PAGE_WALK_CONTROL_HIGH",
        vspace.get("PAGE_WALK_CONTROL_HIGH"),
        0x25e,
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
    require_regex(
        errors,
        vspace_rs,
        r"configure_kernel_direct_map\(\);\s*"
        r"configure_page_walk\(\);\s*"
        r"csr::dbar\(\);\s*"
        r"if\s+current_satp\(\)\s*==\s*satp_val",
        "LoongArch page-walk/direct-map config barrier before paging decisions",
    )
    require_regex(
        errors,
        vspace_rs,
        r"if\s+current_satp\(\)\s*==\s*satp_val\s*\{\s*"
        r"enable_paging\(\);\s*"
        r"csr::dbar\(\);\s*"
        r"(?:csr::sfence_vma_all\(\);\s*)?"
        r"return;",
        "LoongArch same-root paging enable barrier",
    )
    require_regex(
        errors,
        vspace_rs,
        r"csr::set_pgdl\(\(satp_val\s*&\s*!0xfffu64\)\s*as\s*usize\);\s*"
        r"csr::set_asid\(\(satp_val\s*&\s*csr::ASID_MASK\s*as\s*u64\)\s*as\s*usize\);\s*"
        r"csr::dbar\(\);\s*"
        r"enable_paging\(\);",
        "LoongArch PGDL/ASID switch barrier before paging enable",
    )
    require_regex(
        errors,
        vspace_rs,
        r"pub\s+fn\s+set_current_vspace_root\(\)\s*\{\s*"
        r"let\s+current\s*=\s*crate::object::tcb::current\(\);"
        r"\s*if\s+!try_switch_to_tcb_root\(current\)\s*\{\s*"
        r"switch_to_kernel_root\(\);\s*\}\s*\}",
        "LoongArch current-thread VSpace root fallback",
    )
    require_regex(
        errors,
        vspace_rs,
        r"fn\s+try_switch_to_tcb_root\(tcb:\s*\*const\s+crate::object::tcb::Tcb\)\s*->\s*bool\s*\{.*?"
        r"vspace_cap_snapshot\(tcb\);.*?"
        r"vroot\.tag\(\)\s*!=\s*Some\(CapTag::PageTable\).*?"
        r"let\s+root_kva\s*=\s*vroot\.page_table_base_ptr\(\);.*?"
        r"let\s+asid\s*=\s*vroot\.page_table_mapped_asid\(\);.*?"
        r"crate::object::asid::lookup\(asid\)\s*!=\s*root_kva.*?"
        r"let\s+new_satp\s*=\s*satp_from_kva\(root_kva,\s*asid\s+as\s+u64\);.*?"
        r"switch_satp\(new_satp\);.*?"
        r"true",
        "LoongArch TCB VSpace cap validation before root switch",
    )
    require_regex(
        errors,
        trap_rs,
        r"if\s+next\s*!=\s*cur\s*\{.*?"
        r"tcb::set_current\(next\);.*?"
        r"let\s+ctx\s*=\s*unsafe\s*\{\s*tcb::prepare_for_user_restore\(next\)\s*\};.*?"
        r"switch_to_tcb_vspace\(next\);.*?"
        r"return\s+finish_kernel_exit\(ctx,\s*kernel_lock\);",
        "LoongArch scheduler switch changes VSpace before user restore",
    )
    require_regex(
        errors,
        trap_rs,
        r"fn\s+kernel_exit_after_remote_stall\([^)]*\)\s*->\s*\*mut\s+UserContext\s*\{.*?"
        r"tcb::set_current\(next\);.*?"
        r"let\s+ctx\s*=\s*unsafe\s*\{\s*tcb::prepare_for_user_restore\(next\)\s*\};.*?"
        r"switch_to_tcb_vspace\(next\);.*?"
        r"return\s+finish_kernel_exit\(ctx,\s*kernel_lock\);",
        "LoongArch remote-stall exit changes VSpace before user restore",
    )
    require_text(errors, vspace_rs, "csr::set_stlbps(PAGE_SHIFT)", "STLB page-size setup")
    require_text(
        errors,
        vspace_rs,
        "csr::set_pwch(PAGE_WALK_CONTROL_HIGH)",
        "PWCH top-level page-walk setup",
    )
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
    smp_rs = ROOT_DIR / "kernel" / "src" / "kernel" / "smp.rs"
    require_regex(
        errors,
        smp_rs,
        r"pub\s+fn\s+publish_kernel_satp\(satp:\s*u64\)\s*\{\s*"
        r"KERNEL_SATP\.store\(satp,\s*Ordering::Release\);"
        r"\s*#\[cfg\(target_arch\s*=\s*\"loongarch64\"\)\]\s*"
        r"crate::arch::current::csr::dbar\(\);",
        "LoongArch kernel root publish barrier",
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
    elif target.name == "x86_64":
        pass
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
