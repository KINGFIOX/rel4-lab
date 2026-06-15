#!/usr/bin/env python3
"""Audit SMP remote-operation invariants used by architecture backends."""

from __future__ import annotations

import argparse
import re
import sys
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parent))
from target_config import target_from_env
from tool_common import ROOT_DIR, log


PREFIX = "audit-smp-abi"


def require_text(errors: list[str], path: Path, text: str, description: str) -> None:
    if text not in path.read_text():
        errors.append(f"{path.relative_to(ROOT_DIR)} is missing {description}: {text}")


def require_regex(errors: list[str], path: Path, pattern: str, description: str) -> None:
    if re.search(pattern, path.read_text(), re.S) is None:
        errors.append(f"{path.relative_to(ROOT_DIR)} is missing {description}")


def audit_common_smp(errors: list[str]) -> None:
    smp_rs = ROOT_DIR / "kernel" / "src" / "kernel" / "smp.rs"
    require_text(
        errors,
        smp_rs,
        "REMOTE_STALL_OP.store(REMOTE_OP_STALL_TCB, Ordering::Release)",
        "remote op reset to TCB stall",
    )
    require_regex(
        errors,
        smp_rs,
        r"pub\s+fn\s+remote_tcb_stall\([^)]*\)\s*\{"
        r"(?=.*current_core_of_tcb\(tcb\))"
        r"(?=.*remote_core_op\(core,\s*REMOTE_OP_STALL_TCB,\s*tcb\s+as\s+usize\))",
        "remote TCB stall dispatch through remote_core_op",
    )
    require_regex(
        errors,
        smp_rs,
        r"let\s+stalled_current\s*=\s*target\s*!=\s*0\s*&&\s*"
        r"hart\.current_tcb\.load\(Ordering::Acquire\)\s*==\s*target",
        "remote TCB stall current-TCB match",
    )
    require_text(
        errors,
        smp_rs,
        "(*hart.trap_scratch.get()).user_context = 0;",
        "remote TCB stall user-context clearing",
    )
    require_text(
        errors,
        smp_rs,
        "RemoteCoreOpResult::StalledCurrent",
        "remote TCB stall result",
    )


def audit_riscv64(errors: list[str]) -> None:
    smp_rs = ROOT_DIR / "kernel" / "src" / "kernel" / "smp.rs"
    sbi_rs = ROOT_DIR / "kernel" / "src" / "arch" / "riscv64" / "sbi.rs"
    require_text(errors, sbi_rs, "pub const SUPPORTS_REMOTE_IPI: bool = true;", "SBI IPI")
    require_text(
        errors,
        sbi_rs,
        "pub const SUPPORTS_REMOTE_TLB_FLUSH: bool = true;",
        "SBI RFENCE",
    )
    require_regex(
        errors,
        smp_rs,
        r"#\[cfg\(target_arch\s*=\s*\"riscv64\"\)\]\s*"
        r"fn\s+remote_sfence_vma_core\([^)]*\).*?remote_sfence_vma\(1,\s*hart_id,\s*0,\s*0\)",
        "RISC-V remote sfence.vma SBI path",
    )
    require_regex(
        errors,
        smp_rs,
        r"#\[cfg\(target_arch\s*=\s*\"riscv64\"\)\]\s*"
        r"fn\s+remote_sfence_vma_asid_core\([^)]*\).*?"
        r"remote_sfence_vma_asid\(1,\s*hart_id,\s*0,\s*0,\s*asid\)",
        "RISC-V remote sfence.vma.asid SBI path",
    )


def audit_loongarch64(errors: list[str]) -> None:
    smp_rs = ROOT_DIR / "kernel" / "src" / "kernel" / "smp.rs"
    irq_rs = ROOT_DIR / "kernel" / "src" / "arch" / "loongarch64" / "irq.rs"
    sbi_rs = ROOT_DIR / "kernel" / "src" / "arch" / "loongarch64" / "sbi.rs"
    trap_rs = ROOT_DIR / "kernel" / "src" / "arch" / "loongarch64" / "trap.rs"
    require_text(errors, sbi_rs, "pub const SUPPORTS_REMOTE_IPI: bool = true;", "IOCSR IPI")
    require_text(
        errors,
        sbi_rs,
        "pub const SUPPORTS_REMOTE_TLB_FLUSH: bool = false;",
        "no SBI RFENCE on LoongArch",
    )
    require_text(errors, sbi_rs, "IOCSR_IPI_SEND", "IOCSR IPI send register")
    require_text(errors, sbi_rs, "IPI_SEND_ACTION_RESCHEDULE", "IPI reschedule action")
    require_regex(
        errors,
        sbi_rs,
        r"pub\s+fn\s+init_ipi\(\)\s*\{\s*"
        r"csr::iocsr_write64\(IOCSR_IPI_CLEAR,\s*u64::MAX\);"
        r"\s*csr::iocsr_write64\(IOCSR_IPI_EN,\s*u64::MAX\);"
        r"\s*csr::dbar\(\);",
        "LoongArch clears stale IPI state before enable",
    )
    require_regex(
        errors,
        sbi_rs,
        r"pub\s+fn\s+ack_ipi\(\)\s*->\s*bool\s*\{.*?"
        r"csr::iocsr_write64\(IOCSR_IPI_CLEAR,\s*pending\);"
        r"\s*csr::dbar\(\);",
        "LoongArch IPI acknowledgement write barrier",
    )
    require_regex(
        errors,
        sbi_rs,
        r"pub\s+fn\s+send_ipi\([^)]*\)\s*->\s*SbiRet\s*\{.*?"
        r"csr::iocsr_write64\(\s*IOCSR_IPI_SEND,.*?"
        r"csr::dbar\(\);\s*OK",
        "LoongArch IPI send write barrier",
    )
    require_regex(
        errors,
        smp_rs,
        r"#\[cfg\(target_arch\s*=\s*\"loongarch64\"\)\]\s*"
        r"fn\s+remote_sfence_vma_core\([^)]*\)\s*\{\s*"
        r"remote_core_op\(core,\s*REMOTE_OP_FLUSH_VMA_ALL,\s*0\);",
        "LoongArch remote full TLB flush through IPI remote op",
    )
    require_regex(
        errors,
        smp_rs,
        r"#\[cfg\(target_arch\s*=\s*\"loongarch64\"\)\]\s*"
        r"fn\s+remote_sfence_vma_asid_core\([^)]*\)\s*\{\s*"
        r"remote_core_op\(core,\s*REMOTE_OP_FLUSH_VMA_ASID,\s*asid\);",
        "LoongArch remote ASID TLB flush through IPI remote op",
    )
    require_regex(
        errors,
        smp_rs,
        r"pub\s+fn\s+release_secondary_harts\(\)\s*\{\s*"
        r"SECONDARY_BOOT_READY\.store\(SECONDARY_BOOT_READY_MAGIC,\s*Ordering::Release\);"
        r"\s*#\[cfg\(target_arch\s*=\s*\"loongarch64\"\)\]\s*"
        r"crate::arch::current::csr::dbar\(\);",
        "LoongArch secondary-hart release write barrier",
    )
    require_regex(
        errors,
        smp_rs,
        r"fn\s+remote_core_op\(core:\s*usize,\s*op:\s*usize,\s*target_value:\s*usize\)\s*\{.*?"
        r"REMOTE_STALL_TARGET_VALUE\.store\(target_value,\s*Ordering::Release\);"
        r"\s*REMOTE_STALL_OP\.store\(op,\s*Ordering::Release\);"
        r"\s*REMOTE_STALL_DONE_MASK\.store\(0,\s*Ordering::Release\);"
        r"\s*REMOTE_STALL_PENDING_MASK\.store\(bit,\s*Ordering::Release\);"
        r"\s*#\[cfg\(target_arch\s*=\s*\"loongarch64\"\)\]\s*"
        r"crate::arch::current::csr::dbar\(\);"
        r"\s*wake_core\(core\);",
        "LoongArch remote op publish barrier before IPI",
    )
    require_regex(
        errors,
        smp_rs,
        r"REMOTE_OP_FLUSH_VMA_ALL\s*=>\s*\{\s*"
        r"crate::arch::current::csr::sfence_vma_all\(\);",
        "remote full TLB flush service",
    )
    require_regex(
        errors,
        smp_rs,
        r"REMOTE_OP_FLUSH_VMA_ASID\s*=>\s*\{\s*"
        r"crate::arch::current::csr::sfence_vma_asid\(target\);",
        "remote ASID TLB flush service",
    )
    require_text(
        errors,
        smp_rs,
        "#[cfg(target_arch = \"loongarch64\")]\n    crate::arch::current::sbi::ack_ipi();",
        "LoongArch remote op IPI acknowledgement",
    )
    require_regex(
        errors,
        smp_rs,
        r"fn\s+complete_remote_core_op\(bit:\s*usize\)\s*\{\s*"
        r"#\[cfg\(target_arch\s*=\s*\"loongarch64\"\)\]\s*"
        r"crate::arch::current::sbi::ack_ipi\(\);"
        r"\s*#\[cfg\(target_arch\s*=\s*\"loongarch64\"\)\]\s*"
        r"crate::arch::current::csr::dbar\(\);"
        r"\s*REMOTE_STALL_DONE_MASK\.fetch_or\(bit,\s*Ordering::AcqRel\);",
        "LoongArch remote op completion barrier before done bit",
    )
    require_regex(
        errors,
        trap_rs,
        r"if\s+ipi_pending\(estat\)\s*\{.*?"
        r"service_pending_remote_core_op\(\).*?"
        r"RemoteCoreOpResult::StalledCurrent",
        "LoongArch trap IPI remote-op service",
    )
    require_regex(
        errors,
        irq_rs,
        r"pub\s+fn\s+local_irq_save\(\)\s*->\s*bool\s*\{.*?"
        r"super::csr::set_crmd\(crmd\s*&\s*!CRMD_IE\);"
        r"\s*super::csr::dbar\(\);",
        "LoongArch local IRQ disable barrier",
    )
    require_regex(
        errors,
        irq_rs,
        r"pub\s+fn\s+local_irq_restore\(irq_was_enabled:\s*bool\)\s*\{.*?"
        r"super::csr::set_crmd\(crmd\s*\|\s*CRMD_IE\);.*?"
        r"super::csr::set_crmd\(crmd\s*&\s*!CRMD_IE\);.*?"
        r"super::csr::dbar\(\);",
        "LoongArch local IRQ restore barrier",
    )


def main(argv: list[str]) -> int:
    parser = argparse.ArgumentParser(description="Check SMP remote-operation source invariants.")
    parser.parse_args(argv)

    target = target_from_env(PREFIX)
    errors: list[str] = []
    audit_common_smp(errors)
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

    print(f"PASS: {target.name} SMP remote-operation invariants")
    return 0


if __name__ == "__main__":
    raise SystemExit(main(sys.argv[1:]))
