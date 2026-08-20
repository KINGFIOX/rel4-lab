#!/usr/bin/env python3
"""Audit SMP remote-operation invariants used by architecture backends."""

from __future__ import annotations

import argparse
import re
import sys
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parent))
from kernel_arch_paths import arch_dir, ipi_rs, smp_mod_rs
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
        r"(?=.*remote_core_op\(core,\s*REMOTE_OP_STALL_TCB,\s*tcb\.kva\(\)\s+as\s+usize\))",
        "remote TCB stall dispatch through remote_core_op",
    )
    require_regex(
        errors,
        smp_rs,
        r"let\s+stalled_current\s*=\s*target\s*!=\s*0\s*&&\s*"
        r"cpu\.current_tcb\.load\(Ordering::Acquire\)\s*==\s*target",
        "remote TCB stall current-TCB match",
    )
    require_text(
        errors,
        smp_rs,
        "(*cpu.trap_scratch.get()).user_context = 0;",
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
        "ipi::remote_sfence_vma(1, cpu_id, 0, 0).error",
        "RISC-V remote full TLB flush facade",
    )
    require_text(
        errors,
        riscv_smp_rs,
        "ipi::remote_sfence_vma_asid(1, cpu_id, 0, 0, asid).error",
        "RISC-V remote ASID TLB flush facade",
    )
    require_regex(
        errors,
        smp_rs,
        r"fn\s+remote_tlb_flush_core\([^)]*\).*?"
        r"crate::arch::current::smp::remote_tlb_flush_all\(cpu_id\)",
        "remote TLB flush IPI path",
    )
    require_regex(
        errors,
        smp_rs,
        r"fn\s+remote_tlb_flush_asid_core\([^)]*\).*?"
        r"crate::arch::current::smp::remote_tlb_flush_asid\(cpu_id,\s*asid\)",
        "remote ASID TLB flush IPI path",
    )


def main(argv: list[str]) -> int:
    parser = argparse.ArgumentParser(description="Check SMP remote-operation source invariants.")
    parser.parse_args(argv)

    target = target_from_env(PREFIX)
    errors: list[str] = []
    audit_common_smp(errors)
    if target.name == "riscv64":
        audit_riscv64(errors)
    elif target.name == "x86_64":
        x86_ipi = ROOT_DIR / "kernel" / "src" / "arch" / "x86_64" / "smp" / "ipi.rs"
        require_text(errors, x86_ipi, "pub const SUPPORTS_REMOTE_IPI: bool = true;", "x2APIC IPI")
        require_text(
            errors,
            x86_ipi,
            "pub const SUPPORTS_REMOTE_TLB_FLUSH: bool = true;",
            "x2APIC TLB shootdown",
        )
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
