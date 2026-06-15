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


def require_regex(errors: list[str], path: Path, pattern: str, description: str) -> None:
    if re.search(pattern, path.read_text(), re.S) is None:
        errors.append(f"{path.relative_to(ROOT_DIR)} is missing {description}")


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


def audit_loongarch_gpr_save_restore(errors: list[str], asm_path: Path) -> int:
    direct_saves = {
        "zero": 0,
        "ra": 1,
        "tp": 2,
        "a0": 4,
        "a1": 5,
        "a2": 6,
        "a3": 7,
        "a4": 8,
        "a5": 9,
        "a6": 10,
        "a7": 11,
        "t3": 15,
        "t4": 16,
        "t5": 17,
        "t6": 18,
        "t7": 19,
        "t8": 20,
        "r21": 21,
        "fp": 22,
        "s0": 23,
        "s1": 24,
        "s2": 25,
        "s3": 26,
        "s4": 27,
        "s5": 28,
        "s6": 29,
        "s7": 30,
        "s8": 31,
    }
    direct_restores = {
        "ra": 1,
        "tp": 2,
        "sp": 3,
        "a0": 4,
        "a1": 5,
        "a2": 6,
        "a3": 7,
        "a4": 8,
        "a5": 9,
        "a6": 10,
        "a7": 11,
        "t0": 12,
        "t1": 13,
        "t2": 14,
        "t3": 15,
        "t4": 16,
        "t5": 17,
        "t6": 18,
        "t7": 19,
        "t8": 20,
        "r21": 21,
        "fp": 22,
        "s0": 23,
        "s1": 24,
        "s2": 25,
        "s3": 26,
        "s4": 27,
        "s5": 28,
        "s6": 29,
        "s7": 30,
        "s8": 31,
    }

    for reg, index in direct_saves.items():
        require_regex(
            errors,
            asm_path,
            rf"\bst\.d\s+\${reg},\s+\$sp,\s+{index}\*8\b",
            f"LoongArch save of ${reg} into UserContext.regs[{index}]",
        )
    for reg, index in direct_restores.items():
        require_regex(
            errors,
            asm_path,
            rf"\bld\.d\s+\${reg},\s+\$t0,\s+{index}\*8\b",
            f"LoongArch restore of ${reg} from UserContext.regs[{index}]",
        )

    scratch_moves = {
        "sp": ("TRAP_SCRATCH_SAVED_USER_SP", 3),
        "t0": ("CSR_KS0", 12),
        "t1": ("TRAP_SCRATCH_SAVED_USER_T1", 13),
        "t2": ("TRAP_SCRATCH_SAVED_USER_T2", 14),
    }
    for reg, (source, index) in scratch_moves.items():
        if source == "CSR_KS0":
            pattern = rf"\bcsrrd\s+\$t1,\s+{source}\s+st\.d\s+\$t1,\s+\$sp,\s+{index}\*8\b"
        else:
            pattern = (
                rf"\bld\.d\s+\$t1,\s+\$t0,\s+{source}\s+"
                rf"st\.d\s+\$t1,\s+\$sp,\s+{index}\*8\b"
            )
        require_regex(
            errors,
            asm_path,
            pattern,
            f"LoongArch scratch save of ${reg} into UserContext.regs[{index}]",
        )

    require_regex(
        errors,
        asm_path,
        r"\bld\.d\s+\$t0,\s+\$t0,\s+12\*8\s+ertn\b",
        "LoongArch restores user $t0 last before ertn",
    )
    return len(direct_saves) + len(direct_restores) + len(scratch_moves) + 1


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
        "TICLR_CLR_TIMER",
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
        "TICLR_CLR_TIMER": 1 << 0,
        "EXCCODE_INTERRUPT": 0,
        "EXCCODE_SYSCALL": 11,
    }
    for name, expected in expected_trap_values.items():
        require_equal(errors, name, trap_consts.get(name), expected)

    require_regex(
        errors,
        trap_rs,
        r"fn\s+clear_timer_interrupt\(\)\s*\{\s*csr::set_ticlr\(TICLR_CLR_TIMER\);\s*\}",
        "named LoongArch timer-clear helper",
    )
    require_regex(
        errors,
        trap_rs,
        r"pub\s+fn\s+init_timer\(\)\s*\{.*?csr::set_ecfg\(csr::ecfg\(\)\s*\|\s*ECFG_LIE_TIMER\)",
        "LoongArch timer interrupt enable in init_timer",
    )
    require_regex(
        errors,
        trap_rs,
        r"fn\s+program_next_timer\(\)\s*\{.*?crate::kernel::smp::set_next_timer_deadline\(deadline\);.*?csr::set_tcfg\(\(initval\s*<<\s*TCFG_INITVAL_SHIFT\)\s*\|\s*TCFG_ENABLE\)",
        "LoongArch timer reprogramming path",
    )

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
        extra_checked += audit_loongarch_gpr_save_restore(errors, asm_path)

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
