#!/usr/bin/env python3
"""Audit trap assembly layout constants against Rust layout assertions."""

from __future__ import annotations

import argparse
import re
import sys
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parent))
from kernel_arch_paths import trap_asm, trap_rs
from target_config import target_from_env
from tool_common import ROOT_DIR, die, log


PREFIX = "audit-trap-layout"
WORD_BYTES = 8

EQU_RE = re.compile(r"^\s*\.equ\s+([A-Za-z0-9_]+)\s*,\s*(.+?)\s*(?://.*)?$")
OFFSET_RE = re.compile(
    r"offset_of!\((?P<ty>[A-Za-z0-9_:]+),\s*(?P<field>[A-Za-z0-9_]+)\)\s*==\s*(?P<expr>.+?)\);"
)
TRAP_SCRATCH_FIELDS = {
    "kernel_stack_top": "TRAP_SCRATCH_KERNEL_STACK_TOP",
    "user_context": "TRAP_SCRATCH_USER_CONTEXT",
    "saved_user_sp": "TRAP_SCRATCH_SAVED_USER_SP",
    "saved_user_t1": "TRAP_SCRATCH_SAVED_USER_T1",
    "saved_user_t2": "TRAP_SCRATCH_SAVED_USER_T2",
}


def eval_expr(expr: str, symbols: dict[str, int] | None = None) -> int:
    text = expr.split("//", 1)[0].strip()
    text = text.replace("size_of::<usize>()", str(WORD_BYTES))
    symbols = symbols or {}
    for name, value in sorted(symbols.items(), key=lambda item: len(item[0]), reverse=True):
        text = re.sub(rf"\b{re.escape(name)}\b", str(value), text)
    text = re.sub(r"\bas\s+(?:u8|u16|u32|u64|usize|i32|i64|isize)\b", "", text)
    text = text.replace("_", "")
    if not re.fullmatch(r"[0-9A-Fa-fxXbBoO()+*\-/<>|& \t]+", text):
        die(PREFIX, f"unsupported constant expression: {expr}")
    return int(eval(text, {"__builtins__": {}}, {}))


def parse_equ(path: Path) -> dict[str, int]:
    values: dict[str, int] = {}
    for line in path.read_text().splitlines():
        match = EQU_RE.match(line)
        if not match:
            continue
        name, expr = match.groups()
        values[name] = eval_expr(expr, values)
    return values


def parse_rust_offsets(path: Path, types: set[str]) -> dict[tuple[str, str], int]:
    offsets: dict[tuple[str, str], int] = {}
    for line in path.read_text().splitlines():
        match = OFFSET_RE.search(line)
        if not match:
            continue
        if match.group("ty") not in types:
            continue
        offsets[(match.group("ty"), match.group("field"))] = eval_expr(match.group("expr"))
    return offsets


def require_equal(errors: list[str], name: str, got: int | None, expected: int | None) -> None:
    if got is None:
        errors.append(f"missing assembly constant {name}")
        return
    if expected is None:
        errors.append(f"missing Rust offset assertion for {name}")
        return
    if got != expected:
        errors.append(f"{name}={got:#x}, expected {expected:#x}")


def audit_trap_scratch(errors: list[str], asm_equ: dict[str, int], rust_offsets) -> None:
    for field, equ_name in TRAP_SCRATCH_FIELDS.items():
        require_equal(
            errors,
            equ_name,
            asm_equ.get(equ_name),
            rust_offsets.get(("TrapScratch", field)),
        )


def main(argv: list[str]) -> int:
    parser = argparse.ArgumentParser(
        description="Check trap.S layout constants against Rust offset_of assertions."
    )
    parser.parse_args(argv)

    target = target_from_env(PREFIX)
    asm_path = trap_asm(target.name)
    trap_rs_path = trap_rs(target.name)
    smp_rs_path = ROOT_DIR / "kernel" / "src" / "kernel" / "smp.rs"
    if not asm_path.is_file():
        if target.name == "x86_64":
            print("PASS: x86_64 trap layout audit skipped; backend is staged (no trap yet)")
            return 0
        die(PREFIX, f"trap assembly not found: {asm_path}")
    if not trap_rs_path.is_file():
        die(PREFIX, f"trap Rust source not found: {trap_rs_path}")

    asm_equ = parse_equ(asm_path)
    rust_offsets = {
        **parse_rust_offsets(smp_rs_path, {"TrapScratch"}),
        **parse_rust_offsets(trap_rs_path, {"UserContext", "TrapRecord"}),
    }
    errors: list[str] = []
    audit_trap_scratch(errors, asm_equ, rust_offsets)

    if errors:
        for error in errors:
            log(PREFIX, f"FAIL: {error}")
        return 1

    print(f"PASS: {target.name} trap layout constants checked={len(TRAP_SCRATCH_FIELDS)}")
    return 0


if __name__ == "__main__":
    raise SystemExit(main(sys.argv[1:]))
