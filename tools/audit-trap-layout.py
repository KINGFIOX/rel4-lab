#!/usr/bin/env python3
"""Audit trap assembly layout constants against Rust layout assertions."""

from __future__ import annotations

import argparse
import re
import sys
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parent))
from target_config import target_from_env
from tool_common import ROOT_DIR, die, log


PREFIX = "audit-trap-layout"
WORD_BYTES = 8

EQU_RE = re.compile(r"^\s*\.equ\s+([A-Za-z0-9_]+)\s*,\s*(.+?)\s*(?://.*)?$")
OFFSET_RE = re.compile(
    r"offset_of!\((?P<ty>[A-Za-z0-9_:]+),\s*(?P<field>[A-Za-z0-9_]+)\)\s*==\s*(?P<expr>.+?)\);"
)
RUST_CONST_RE = re.compile(
    r"(?:pub\s+)?const\s+([A-Z0-9_]+)\s*:\s*[^=]+=\s*(?P<expr>.*?);",
    re.S,
)

TRAP_SCRATCH_FIELDS = {
    "kernel_stack_top": "TRAP_SCRATCH_KERNEL_STACK_TOP",
    "user_context": "TRAP_SCRATCH_USER_CONTEXT",
    "saved_user_sp": "TRAP_SCRATCH_SAVED_USER_SP",
    "saved_user_t1": "TRAP_SCRATCH_SAVED_USER_T1",
    "saved_user_t2": "TRAP_SCRATCH_SAVED_USER_T2",
}

LOONGARCH_USER_CONTEXT_FIELDS = {
    "pc": "USER_CONTEXT_PC",
    "sstatus": "USER_CONTEXT_SSTATUS",
    "restart_pc": "USER_CONTEXT_RESTART_PC",
    "trap_record": "USER_CONTEXT_TRAP_RECORD",
}

LOONGARCH_TRAP_RECORD_FIELDS = {
    "era": "USER_CONTEXT_TRAP_RECORD_ERA",
    "prmd": "USER_CONTEXT_TRAP_RECORD_PRMD",
    "estat": "USER_CONTEXT_TRAP_RECORD_ESTAT",
    "badv": "USER_CONTEXT_TRAP_RECORD_BADV",
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


def parse_rust_consts(path: Path, names: set[str]) -> dict[str, int]:
    values: dict[str, int] = {}
    for match in RUST_CONST_RE.finditer(path.read_text()):
        name = match.group(1)
        if name not in names:
            continue
        try:
            values[name] = eval_expr(match.group("expr"), values)
        except Exception as exc:
            die(PREFIX, f"could not evaluate {name} in {path}: {exc}")
    return values


def require_equal(errors: list[str], name: str, got: int | None, expected: int | None) -> None:
    if got is None:
        errors.append(f"missing assembly constant {name}")
        return
    if expected is None:
        errors.append(f"missing Rust offset assertion for {name}")
        return
    if got != expected:
        errors.append(f"{name}={got:#x}, expected {expected:#x}")


def require_present(
    errors: list[str], context: str, values: dict[str, int], name: str
) -> int | None:
    value = values.get(name)
    if value is None:
        errors.append(f"missing {context} constant {name}")
    return value


def audit_trap_scratch(errors: list[str], asm_equ: dict[str, int], rust_offsets) -> None:
    for field, equ_name in TRAP_SCRATCH_FIELDS.items():
        require_equal(
            errors,
            equ_name,
            asm_equ.get(equ_name),
            rust_offsets.get(("TrapScratch", field)),
        )


def audit_loongarch_user_context(errors: list[str], asm_equ: dict[str, int], rust_offsets) -> None:
    trap_record_base = rust_offsets.get(("UserContext", "trap_record"))
    for field, equ_name in LOONGARCH_USER_CONTEXT_FIELDS.items():
        require_equal(
            errors,
            equ_name,
            asm_equ.get(equ_name),
            rust_offsets.get(("UserContext", field)),
        )
    for field, equ_name in LOONGARCH_TRAP_RECORD_FIELDS.items():
        field_offset = rust_offsets.get(("TrapRecord", field))
        expected = (
            None
            if trap_record_base is None or field_offset is None
            else trap_record_base + field_offset
        )
        require_equal(errors, equ_name, asm_equ.get(equ_name), expected)


def audit_loongarch_trap_abi(errors: list[str], asm_equ: dict[str, int]) -> int:
    csr_rs = ROOT_DIR / "kernel" / "src" / "arch" / "loongarch64" / "csr.rs"
    irq_rs = ROOT_DIR / "kernel" / "src" / "arch" / "loongarch64" / "irq.rs"
    trap_rs = ROOT_DIR / "kernel" / "src" / "arch" / "loongarch64" / "trap.rs"

    csr_names = {
        "CSR_PRMD",
        "CSR_ESTAT",
        "CSR_ERA",
        "CSR_BADV",
        "CSR_KS0",
    }
    irq_names = {
        "MAX_IRQ",
        "KERNEL_TIMER_IRQ",
        "ECFG_LIE_EXTIOI0",
        "ECFG_LIE_IPI",
    }
    trap_names = {
        "PRMD_PPLV_MASK",
        "PRMD_PPLV_USER",
        "PRMD_PIE",
        "USER_SSTATUS",
        "ROOTSERVER_SSTATUS",
        "ESTAT_ECODE_SHIFT",
        "ESTAT_ECODE_MASK",
        "ESTAT_ESUBCODE_SHIFT",
        "ESTAT_ESUBCODE_MASK",
        "ESTAT_IS_EXTIOI0",
        "ESTAT_IS_TIMER",
        "ESTAT_IS_IPI",
        "ECFG_LIE_TIMER",
        "TCFG_ENABLE",
        "TCFG_INITVAL_SHIFT",
        "EXCCODE_INTERRUPT",
        "EXCCODE_SYSCALL",
    }
    csr_consts = parse_rust_consts(csr_rs, csr_names)
    irq_consts = parse_rust_consts(irq_rs, irq_names)
    trap_consts = parse_rust_consts(trap_rs, trap_names)

    for name in csr_names:
        require_equal(errors, name, asm_equ.get(name), csr_consts.get(name))
    for name in ("PRMD_PPLV_MASK", "PRMD_PPLV_USER"):
        require_equal(errors, name, asm_equ.get(name), trap_consts.get(name))

    expected_trap_values = {
        "PRMD_PPLV_MASK": 0b11,
        "PRMD_PPLV_USER": 0b11,
        "PRMD_PIE": 1 << 2,
        "ESTAT_ECODE_SHIFT": 16,
        "ESTAT_ECODE_MASK": 0x3f,
        "ESTAT_ESUBCODE_SHIFT": 22,
        "ESTAT_ESUBCODE_MASK": 0x1ff,
        "ESTAT_IS_EXTIOI0": 1 << 2,
        "ESTAT_IS_TIMER": 1 << 11,
        "ESTAT_IS_IPI": 1 << 12,
        "ECFG_LIE_TIMER": 1 << 11,
        "TCFG_ENABLE": 1 << 0,
        "TCFG_INITVAL_SHIFT": 2,
        "EXCCODE_INTERRUPT": 0,
        "EXCCODE_SYSCALL": 11,
    }
    for name, expected in expected_trap_values.items():
        require_equal(errors, name, trap_consts.get(name), expected)

    user_sstatus = require_present(errors, "trap", trap_consts, "USER_SSTATUS")
    rootserver_sstatus = require_present(errors, "trap", trap_consts, "ROOTSERVER_SSTATUS")
    prmd_user = require_present(errors, "trap", trap_consts, "PRMD_PPLV_USER")
    prmd_pie = require_present(errors, "trap", trap_consts, "PRMD_PIE")
    if user_sstatus is not None and prmd_user is not None and prmd_pie is not None:
        require_equal(errors, "USER_SSTATUS", user_sstatus, prmd_user | prmd_pie)
    if rootserver_sstatus is not None and user_sstatus is not None:
        require_equal(errors, "ROOTSERVER_SSTATUS", rootserver_sstatus, user_sstatus)

    require_equal(
        errors,
        "ECFG_LIE_EXTIOI0",
        irq_consts.get("ECFG_LIE_EXTIOI0"),
        trap_consts.get("ESTAT_IS_EXTIOI0"),
    )
    require_equal(
        errors,
        "ECFG_LIE_IPI",
        irq_consts.get("ECFG_LIE_IPI"),
        trap_consts.get("ESTAT_IS_IPI"),
    )
    require_equal(
        errors,
        "ECFG_LIE_TIMER",
        trap_consts.get("ECFG_LIE_TIMER"),
        trap_consts.get("ESTAT_IS_TIMER"),
    )
    require_equal(
        errors,
        "KERNEL_TIMER_IRQ",
        irq_consts.get("KERNEL_TIMER_IRQ"),
        irq_consts.get("MAX_IRQ"),
    )
    require_equal(errors, "MAX_IRQ", irq_consts.get("MAX_IRQ"), 256)
    return len(csr_names) + 2 + len(expected_trap_values) + 7


def main(argv: list[str]) -> int:
    parser = argparse.ArgumentParser(
        description="Check trap.S layout constants against Rust offset_of assertions."
    )
    parser.parse_args(argv)

    target = target_from_env(PREFIX)
    asm_path = ROOT_DIR / "kernel" / "src" / "arch" / target.name / "trap.S"
    trap_rs_path = ROOT_DIR / "kernel" / "src" / "arch" / target.name / "trap.rs"
    smp_rs_path = ROOT_DIR / "kernel" / "src" / "kernel" / "smp.rs"
    if not asm_path.is_file():
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
    extra_checked = 0
    if target.name == "loongarch64":
        audit_loongarch_user_context(errors, asm_equ, rust_offsets)
        extra_checked += audit_loongarch_trap_abi(errors, asm_equ)

    if errors:
        for error in errors:
            log(PREFIX, f"FAIL: {error}")
        return 1

    checked = len(TRAP_SCRATCH_FIELDS)
    if target.name == "loongarch64":
        checked += len(LOONGARCH_USER_CONTEXT_FIELDS) + len(LOONGARCH_TRAP_RECORD_FIELDS)
    checked += extra_checked
    print(f"PASS: {target.name} trap layout constants checked={checked}")
    return 0


if __name__ == "__main__":
    raise SystemExit(main(sys.argv[1:]))
