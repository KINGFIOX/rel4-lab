#!/usr/bin/env python3
"""Audit SMP remote-operation invariants used by architecture backends."""

from __future__ import annotations

import argparse
import re
import sys
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parent))
from kernel_arch_paths import arch_dir, ipi_rs, irq_rs, smp_mod_rs, trap_rs
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
    mod_rs = arch_dir("riscv64") / "mod.rs"
    riscv_smp_rs = smp_mod_rs("riscv64")
    riscv_ipi_rs = ipi_rs("riscv64")
    require_text(errors, mod_rs, "pub mod smp;", "RISC-V SMP module")
    require_text(errors, riscv_smp_rs, "pub mod ipi;", "RISC-V IPI module")
    require_text(errors, riscv_ipi_rs, "pub const SUPPORTS_REMOTE_IPI: bool = true;", "SBI IPI")
    require_text(
        errors,
        riscv_ipi_rs,
        "pub const SUPPORTS_REMOTE_TLB_FLUSH: bool = true;",
        "SBI RFENCE",
    )
    require_text(
        errors,
        riscv_smp_rs,
        "ipi::remote_sfence_vma(1, hart_id, 0, 0).error",
        "RISC-V remote full TLB flush facade",
    )
    require_text(
        errors,
        riscv_smp_rs,
        "ipi::remote_sfence_vma_asid(1, hart_id, 0, 0, asid).error",
        "RISC-V remote ASID TLB flush facade",
    )
    require_regex(
        errors,
        smp_rs,
        r"fn\s+remote_sfence_vma_core\([^)]*\).*?"
        r"crate::arch::current::smp::remote_tlb_flush_all\(hart_id\)",
        "RISC-V remote sfence.vma IPI/RFENCE path",
    )
    require_regex(
        errors,
        smp_rs,
        r"fn\s+remote_sfence_vma_asid_core\([^)]*\).*?"
        r"crate::arch::current::smp::remote_tlb_flush_asid\(hart_id,\s*asid\)",
        "RISC-V remote sfence.vma.asid IPI/RFENCE path",
    )


def audit_loongarch64(errors: list[str]) -> None:
    smp_rs = ROOT_DIR / "kernel" / "src" / "kernel" / "smp.rs"
    loongarch_irq_rs = irq_rs("loongarch64")
    mod_rs = arch_dir("loongarch64") / "mod.rs"
    loongarch_smp_rs = smp_mod_rs("loongarch64")
    loongarch_ipi_rs = ipi_rs("loongarch64")
    loongarch_trap_rs = trap_rs("loongarch64")
    require_text(errors, mod_rs, "pub mod smp;", "LoongArch SMP module")
    require_text(errors, loongarch_smp_rs, "pub mod ipi;", "LoongArch IPI module")
    require_text(errors, loongarch_ipi_rs, "pub const SUPPORTS_REMOTE_IPI: bool = true;", "IOCSR IPI")
    require_text(
        errors,
        loongarch_ipi_rs,
        "pub const SUPPORTS_REMOTE_TLB_FLUSH: bool = false;",
        "no direct RFENCE backend on LoongArch",
    )
    require_text(errors, loongarch_ipi_rs, "IOCSR_IPI_SEND", "IOCSR IPI send register")
    require_text(errors, loongarch_ipi_rs, "IPI_SEND_ACTION_RESCHEDULE", "IPI reschedule action")
    require_regex(
        errors,
        loongarch_ipi_rs,
        r"pub\s+fn\s+init_ipi\(\)\s*\{\s*"
        r"csr::iocsr_write64\(IOCSR_IPI_CLEAR,\s*u64::MAX\);"
        r"\s*csr::iocsr_write64\(IOCSR_IPI_EN,\s*u64::MAX\);"
        r"\s*csr::dbar\(\);",
        "LoongArch clears stale IPI state before enable",
    )
    require_regex(
        errors,
        loongarch_ipi_rs,
        r"pub\s+fn\s+ack_ipi\(\)\s*->\s*bool\s*\{.*?"
        r"csr::iocsr_write64\(IOCSR_IPI_CLEAR,\s*pending\);"
        r"\s*csr::dbar\(\);",
        "LoongArch IPI acknowledgement write barrier",
    )
    require_regex(
        errors,
        loongarch_ipi_rs,
        r"pub\s+fn\s+send_ipi\([^)]*\)\s*->\s*IpiRet\s*\{.*?"
        r"csr::iocsr_write64\(\s*IOCSR_IPI_SEND,.*?"
        r"csr::dbar\(\);\s*OK",
        "LoongArch IPI send write barrier",
    )
    require_regex(
        errors,
        smp_rs,
        r"pub\s+fn\s+release_secondary_harts\(\)\s*\{\s*"
        r"SECONDARY_BOOT_READY\.store\(SECONDARY_BOOT_READY_MAGIC,\s*Ordering::Release\);"
        r"\s*crate::arch::current::machine::full_memory_barrier\(\);",
        "architecture secondary-hart release write barrier facade",
    )
    require_regex(
        errors,
        smp_rs,
        r"fn\s+remote_core_op\(core:\s*usize,\s*op:\s*usize,\s*target_value:\s*usize\)\s*\{.*?"
        r"REMOTE_STALL_TARGET_VALUE\.store\(target_value,\s*Ordering::Release\);"
        r"\s*REMOTE_STALL_OP\.store\(op,\s*Ordering::Release\);"
        r"\s*REMOTE_STALL_DONE_MASK\.store\(0,\s*Ordering::Release\);"
        r"\s*REMOTE_STALL_PENDING_MASK\.store\(bit,\s*Ordering::Release\);"
        r"\s*crate::arch::current::machine::full_memory_barrier\(\);"
        r"\s*wake_core\(core\);",
        "architecture remote op publish barrier before IPI",
    )
    require_regex(
        errors,
        smp_rs,
        r"REMOTE_OP_FLUSH_VMA_ALL\s*=>\s*\{\s*"
        r"crate::arch::current::machine::tlb_flush_all\(\);",
        "remote full TLB flush service",
    )
    require_regex(
        errors,
        smp_rs,
        r"REMOTE_OP_FLUSH_VMA_ASID\s*=>\s*\{\s*"
        r"crate::arch::current::machine::tlb_flush_asid\(target\);",
        "remote ASID TLB flush service",
    )
    require_text(
        errors,
        loongarch_smp_rs,
        "ipi::ack_ipi();",
        "LoongArch remote op IPI acknowledgement facade",
    )
    require_regex(
        errors,
        smp_rs,
        r"fn\s+complete_remote_core_op\(bit:\s*usize\)\s*\{\s*"
        r"crate::arch::current::smp::complete_remote_call\(\);"
        r"\s*crate::arch::current::machine::full_memory_barrier\(\);"
        r"\s*REMOTE_STALL_DONE_MASK\.fetch_or\(bit,\s*Ordering::AcqRel\);",
        "architecture remote op completion barrier before done bit",
    )
    require_regex(
        errors,
        loongarch_trap_rs,
        r"if\s+ipi_pending\(estat\)\s*\{.*?"
        r"service_pending_remote_core_op\(\).*?"
        r"RemoteCoreOpResult::StalledCurrent",
        "LoongArch trap IPI remote-op service",
    )
    require_regex(
        errors,
        loongarch_irq_rs,
        r"pub\s+fn\s+local_irq_save\(\)\s*->\s*bool\s*\{.*?"
        r"super::csr::set_crmd\(crmd\s*&\s*!CRMD_IE\);"
        r"\s*super::csr::dbar\(\);",
        "LoongArch local IRQ disable barrier",
    )
    require_regex(
        errors,
        loongarch_irq_rs,
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
    elif target.name == "x86_64":
        pass
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
