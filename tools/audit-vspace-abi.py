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
from kernel_arch_paths import paging_rs
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


TARGET_CFG_RE = re.compile(r'target_arch\s*=\s*"([^"]+)"')


def cfg_matches_target(line: str, target_name: str | None) -> bool:
    if target_name is None:
        return True
    match = TARGET_CFG_RE.search(line)
    return match is None or match.group(1) == target_name


def parse_consts(
    path: Path,
    initial: dict[str, int] | None = None,
    target_name: str | None = None,
) -> dict[str, int]:
    symbols = dict(initial or {})
    filtered_lines: list[str] = []
    pending_include = True
    for line in path.read_text().splitlines():
        stripped = line.strip()
        if stripped.startswith("#[cfg("):
            pending_include = cfg_matches_target(stripped, target_name)
            continue
        if not pending_include:
            if ";" in stripped:
                pending_include = True
            continue
        filtered_lines.append(line)
        if ";" in stripped:
            pending_include = True

    for match in CONST_RE.finditer("\n".join(filtered_lines)):
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


def audit_riscv64(errors: list[str]) -> None:
    abi_consts = parse_consts(
        ROOT_DIR / "kernel" / "src" / "abi" / "constants.rs",
        target_name="riscv64",
    )
    sv39_rs = paging_rs("riscv64")
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
    if target.name == "riscv64":
        audit_riscv64(errors)
    elif target.name == "x86_64":
        print("PASS: x86_64 VSpace ABI audit skipped; backend is staged (no trap yet)")
        return 0
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
