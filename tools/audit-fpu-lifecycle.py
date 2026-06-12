#!/usr/bin/env python3
"""Audit seL4-style RISC-V FPU lifecycle hooks in the Rust kernel sources."""

from __future__ import annotations

import argparse
import re
import sys
from dataclasses import dataclass
from pathlib import Path


@dataclass(frozen=True)
class Check:
    name: str
    path: str
    patterns: tuple[str, ...]
    ordered: bool = False
    forbidden_patterns: tuple[str, ...] = ()


def upstream_fp_reg_patterns(macro: str) -> tuple[str, ...]:
    return tuple(
        rf'{macro}\s+" f{reg},\s*{reg}\*"\s*FP_REG_BYTES\s*"\(%0\)' for reg in range(32)
    )


def rust_fp_reg_patterns(mnemonic: str) -> tuple[str, ...]:
    return tuple(
        rf'"{mnemonic}\s+f{reg},\s*{reg}\*8\(\{{regs\}}\)"' for reg in range(32)
    )


def read_check_text(path: Path) -> str:
    if path.is_dir():
        parts: list[str] = []
        for child in sorted(path.rglob("*")):
            if child.is_file() and child.suffix in {".rs", ".S"}:
                parts.append(f"\n/* {child.relative_to(path)} */\n")
                parts.append(child.read_text(encoding="utf-8"))
        return "".join(parts)
    return path.read_text(encoding="utf-8")


CHECKS: tuple[Check, ...] = (
    Check(
        name="Rust toolchain includes the RISC-V FPU-capable target",
        path="rust-toolchain.toml",
        patterns=(r'targets\s*=\s*\[[^\]]*"riscv64gc-unknown-none-elf"',),
        forbidden_patterns=(r"riscv64imac-unknown-none-elf",),
    ),
    Check(
        name="Cargo default target is RISC-V FPU-capable",
        path=".cargo/config.toml",
        patterns=(
            r'target\s*=\s*"riscv64gc-unknown-none-elf"',
            r"\[target\.riscv64gc-unknown-none-elf\]",
        ),
        ordered=True,
        forbidden_patterns=(r"riscv64imac-unknown-none-elf",),
    ),
    Check(
        name="kernel packer defaults to the RISC-V FPU-capable target",
        path="tools/target_config.py",
        patterns=(
            r'"riscv64":\s*TargetConfig\(',
            r'rust_target="riscv64gc-unknown-none-elf"',
        ),
        ordered=True,
        forbidden_patterns=(r"riscv64imac-unknown-none-elf",),
    ),
    Check(
        name="kernel packer builds the selected Rust target",
        path="tools/pack-image.py",
        patterns=(
            r"rust_target\s*=\s*rust_target_from_env\(target\)",
            r'"cargo",\s*"build",\s*"--release",\s*"--target",\s*rust_target,\s*"-p",\s*"kernel"',
        ),
        ordered=True,
    ),
    Check(
        name="kernel packer pins upstream sel4test RISC-V D/F extensions",
        path="tools/pack-image.py",
        patterns=(
            r'if\s+target\.name\s*==\s*"riscv64"',
            r'values\["KernelRiscvExtD"\]\s*=\s*"ON"',
            r'values\["KernelRiscvExtF"\]\s*=\s*"ON"',
            r'for\s+key\s+in\s+\("KernelRiscvExtD",\s*"KernelRiscvExtF"\)',
            r"key\s+in\s+defaults\s+and\s+cache\.get\(key\)\s*!=\s*defaults\[key\]",
        ),
        ordered=True,
    ),
    Check(
        name="upstream seL4 RISC-V D extension implies FPU support",
        path="third_party/sel4-lab/sel4test/kernel/src/arch/riscv/config.cmake",
        patterns=(
            r"config_option\(KernelRiscvExtF\s+RISCV_EXT_F",
            r"config_option\(KernelRiscvExtD\s+RISCV_EXT_D",
            r"if\(KernelRiscvExtD\)",
            r"set\(KernelRiscvExtF ON\)",
            r"if\(KernelRiscvExtF\)",
            r"set\(KernelHaveFPU ON\)",
        ),
        ordered=True,
    ),
    Check(
        name="kernel FPU instruction audit defaults to the RISC-V FPU-capable target",
        path="tools/audit-kernel-fpu.py",
        patterns=(
            r'DEFAULT_RUST_TARGET\s*=\s*"riscv64gc-unknown-none-elf"',
            r'"cargo",\s*"build",\s*"--release",\s*"--target",\s*args\.target,\s*"-p",\s*"kernel"',
        ),
        ordered=True,
        forbidden_patterns=(r"riscv64imac-unknown-none-elf",),
    ),
    Check(
        name="kernel FPU instruction audit catches explicit FPU CSR operands",
        path="tools/audit-kernel-fpu.py",
        patterns=(
            r"CSR_FPU_REGISTERS\s*=\s*\{",
            r'"fcsr"',
            r'"fflags"',
            r'"frm"',
            r"CSR_MNEMONICS\s*=\s*\{",
            r'"csrr"',
            r'"csrw"',
            r'"csrrw"',
            r"def\s+is_fpu_mnemonic\(mnemonic:\s*str,\s*operands:\s*str\s*=\s*\"\"\)\s*->\s*bool",
            r"if\s+mnemonic\s+not\s+in\s+CSR_MNEMONICS",
            r"re\.search\(rf\"\\b\{re\.escape\(register\)\}\\b\",\s*operands\)",
        ),
        ordered=True,
    ),
    Check(
        name="upstream seL4 local FPU owner switch anchor",
        path="third_party/sel4-lab/sel4test/kernel/src/machine/fpu.c",
        patterns=(
            r"void\s+switchLocalFpuOwner\(tcb_t \*new_owner\)",
            r"enableFpu\(\);",
            r"saveFpuState\(NODE_STATE\(ksCurFPUOwner\)\)",
            r"loadFpuState\(new_owner\)",
            r"disableFpu\(\)",
            r"NODE_STATE\(ksCurFPUOwner\)\s*=\s*new_owner",
        ),
        ordered=True,
    ),
    Check(
        name="upstream seL4 lazy FPU restore anchor",
        path="third_party/sel4-lab/sel4test/kernel/include/machine/fpu.h",
        patterns=(
            r"static\s+inline\s+void\s+FORCE_INLINE\s+lazyFPURestore",
            r"thread->tcbFlags\s*&\s*seL4_TCBFlag_fpuDisabled",
            r"disableFpu\(\)",
            r"nativeThreadUsingFPU\(thread\)",
            r"enableFpu\(\)",
            r"switchLocalFpuOwner\(thread\)",
        ),
        ordered=True,
    ),
    Check(
        name="upstream seL4 switchToThread refreshes lazy FPU state",
        path="third_party/sel4-lab/sel4test/kernel/src/kernel/thread.c",
        patterns=(
            r"void\s+switchToThread\(tcb_t \*thread\)",
            r"Arch_switchToThread\(thread\)",
            r"#ifdef\s+CONFIG_HAVE_FPU",
            r"lazyFPURestore\(thread\)",
            r"#endif\s*/\*\s*CONFIG_HAVE_FPU\s*\*/",
            r"tcbSchedDequeue\(thread\)",
            r"NODE_STATE\(ksCurThread\)\s*=\s*thread",
        ),
        ordered=True,
    ),
    Check(
        name="upstream seL4 native FPU owner is checked on thread affinity core",
        path="third_party/sel4-lab/sel4test/kernel/include/machine/fpu.h",
        patterns=(
            r"static\s+inline\s+bool_t\s+nativeThreadUsingFPU\(tcb_t \*thread\)",
            r"return\s+thread\s*==\s*NODE_STATE_ON_CORE\(ksCurFPUOwner,\s*thread->tcbAffinity\)",
        ),
        ordered=True,
    ),
    Check(
        name="upstream seL4 RISC-V FPU boot init anchor",
        path="third_party/sel4-lab/sel4test/kernel/src/arch/riscv/kernel/boot.c",
        patterns=(
            r"BOOT_CODE\s+static\s+void\s+init_fpu\(void\)",
            r"set_fs_clean\(\)",
            r"write_fcsr\(0\)",
            r"disableFpu\(\)",
            r"BOOT_CODE\s+static\s+void\s+init_cpu\(void\)",
            r"set_fs_off\(\)",
            r"init_fpu\(\)",
        ),
        ordered=True,
    ),
    Check(
        name="upstream seL4 boot clears current FPU owner state",
        path="third_party/sel4-lab/sel4test/kernel/src/kernel/boot.c",
        patterns=(
            r"BOOT_CODE\s+void\s+init_core_state\(tcb_t \*scheduler_action\)",
            r"#ifdef\s+CONFIG_HAVE_FPU",
            r"NODE_STATE\(ksCurFPUOwner\)\s*=\s*NULL",
            r"#endif",
            r"NODE_STATE\(ksSchedulerAction\)\s*=\s*scheduler_action",
        ),
        ordered=True,
    ),
    Check(
        name="upstream seL4 RISC-V FPU access shadow anchor",
        path="third_party/sel4-lab/sel4test/kernel/include/arch/riscv/arch/machine/fpu.h",
        patterns=(
            r"extern\s+bool_t\s+isFPUEnabled\[CONFIG_MAX_NUM_NODES\]",
            r"asm volatile\(\"csrs sstatus, %0\" :: \"rK\"\(SSTATUS_FS_CLEAN\)\)",
            r"static\s+inline\s+void\s+enableFpu\(void\)",
            r"isFPUEnabled\[CURRENT_CPU_INDEX\(\)\]\s*=\s*true",
            r"static\s+inline\s+void\s+disableFpu\(void\)",
            r"isFPUEnabled\[CURRENT_CPU_INDEX\(\)\]\s*=\s*false",
            r"static\s+inline\s+bool_t\s+isFpuEnable\(void\)",
            r"return\s+isFPUEnabled\[CURRENT_CPU_INDEX\(\)\]",
            r"set_tcb_fs_state\(tcb_t \*tcb,\s*bool_t enabled\)",
        ),
        ordered=True,
    ),
    Check(
        name="upstream seL4 RISC-V FPU access toggles are shadow-only",
        path="third_party/sel4-lab/sel4test/kernel/include/arch/riscv/arch/machine/fpu.h",
        patterns=(
            r"static\s+inline\s+void\s+enableFpu\(void\)",
            r"isFPUEnabled\[CURRENT_CPU_INDEX\(\)\]\s*=\s*true",
            r"static\s+inline\s+void\s+disableFpu\(void\)",
            r"isFPUEnabled\[CURRENT_CPU_INDEX\(\)\]\s*=\s*false",
        ),
        ordered=True,
        forbidden_patterns=(
            r"static\s+inline\s+void\s+enableFpu\(void\)\s*\{(?:(?!\n\}).)*"
            r"(?:asm\s+volatile|sstatus|set_fs_)",
            r"static\s+inline\s+void\s+disableFpu\(void\)\s*\{(?:(?!\n\}).)*"
            r"(?:asm\s+volatile|sstatus|set_fs_)",
        ),
    ),
    Check(
        name="upstream seL4 RISC-V FPU access shadow is BSS-backed",
        path="third_party/sel4-lab/sel4test/kernel/src/arch/riscv/machine/fpu.c",
        patterns=(r"bool_t\s+isFPUEnabled\[CONFIG_MAX_NUM_NODES\];",),
    ),
    Check(
        name="upstream seL4 RISC-V TCB FS-state helper clears then conditionally enables FS",
        path="third_party/sel4-lab/sel4test/kernel/include/arch/riscv/arch/machine/fpu.h",
        patterns=(
            r"static\s+inline\s+void\s+set_tcb_fs_state\(tcb_t \*tcb,\s*bool_t enabled\)",
            r"word_t\s+sstatus\s*=\s*getRegister\(tcb,\s*SSTATUS\)",
            r"sstatus\s*&=\s*~SSTATUS_FS",
            r"if\s*\(enabled\)",
            r"sstatus\s*\|=\s*SSTATUS_FS_CLEAN",
            r"setRegister\(tcb,\s*SSTATUS,\s*sstatus\)",
        ),
        ordered=True,
    ),
    Check(
        name="upstream seL4 RISC-V sstatus FS bit encoding anchor",
        path="third_party/sel4-lab/sel4test/kernel/include/arch/riscv/arch/machine/hardware.h",
        patterns=(
            r"#define\s+SSTATUS_SPIE\s+0x00000020",
            r"#define\s+SSTATUS_FS\s+0x00006000",
            r"#define\s+SSTATUS_FS_CLEAN\s+0x00004000",
            r"#define\s+SSTATUS_FS_INITIAL\s+0x00002000",
            r"#define\s+SSTATUS_FS_DIRTY\s+0x00006000",
        ),
        ordered=True,
    ),
    Check(
        name="upstream seL4 RISC-V D FPU state layout anchor",
        path="third_party/sel4-lab/sel4test/kernel/include/arch/riscv/arch/machine/registerset.h",
        patterns=(
            r"SCAUSE\s*=\s*31",
            r"SSTATUS\s*=\s*32",
            r"FaultIP\s*=\s*33",
            r"NextIP\s*=\s*34",
            r"n_contextRegisters",
            r"#define\s+RISCV_NUM_FP_REGS\s+32",
            r"#if\s+defined\(CONFIG_RISCV_EXT_D\)",
            r"typedef\s+uint64_t\s+fp_reg_t;",
            r"typedef\s+struct\s+user_fpu_state",
            r"fp_reg_t\s+regs\[RISCV_NUM_FP_REGS\];",
            r"uint32_t\s+fcsr;",
            r"struct\s+user_context",
            r"word_t\s+registers\[n_contextRegisters\];",
            r"user_fpu_state_t\s+fpuState;",
        ),
        ordered=True,
    ),
    Check(
        name="upstream seL4 RISC-V context init does not pre-enable FPU state",
        path="third_party/sel4-lab/sel4test/kernel/include/arch/riscv/arch/machine/registerset.h",
        patterns=(
            r"static\s+inline\s+void\s+Arch_initContext\(user_context_t \*context\)",
            r"context->registers\[SSTATUS\]\s*=\s*SSTATUS_SPIE",
        ),
        ordered=True,
        forbidden_patterns=(
            r"static\s+inline\s+void\s+Arch_initContext\(user_context_t \*context\)\s*\{"
            r"(?:(?!\n\}).)*(?:fpuState|SSTATUS_FS)",
        ),
    ),
    Check(
        name="upstream seL4 RISC-V fcsr CSR helper anchor",
        path="third_party/sel4-lab/sel4test/kernel/include/arch/riscv/arch/machine.h",
        patterns=(
            r"static\s+inline\s+uint32_t\s+read_fcsr\(void\)",
            r"asm\s+volatile\(\"csrr %0, fcsr\"\s*:\s*\"=r\"\(fcsr\)\)",
            r"return\s+fcsr",
            r"static\s+inline\s+void\s+write_fcsr\(uint32_t value\)",
            r"asm\s+volatile\(\"csrw fcsr, %0\"\s*::\s*\"rK\"\(value\)\)",
        ),
        ordered=True,
    ),
    Check(
        name="upstream seL4 RISC-V FPU state save/load covers f0-f31 and fcsr",
        path="third_party/sel4-lab/sel4test/kernel/include/arch/riscv/arch/machine/fpu.h",
        patterns=(
            r"static\s+inline\s+void\s+saveFpuState\(tcb_t \*thread\)",
            r"set_fs_clean\(\)",
            *upstream_fp_reg_patterns("FS"),
            r"dest->fcsr\s*=\s*read_fcsr\(\)",
            r"static\s+inline\s+void\s+loadFpuState\(const tcb_t \*thread\)",
            r"set_fs_clean\(\)",
            *upstream_fp_reg_patterns("FL"),
            r"write_fcsr\(src->fcsr\)",
        ),
        ordered=True,
    ),
    Check(
        name="upstream seL4 RISC-V restore FS boundary anchor",
        path="third_party/sel4-lab/sel4test/kernel/src/arch/riscv/c_traps.c",
        patterns=(
            r"void\s+VISIBLE\s+NORETURN\s+restore_user_context\(void\)",
            r"set_tcb_fs_state\(NODE_STATE\(ksCurThread\),\s*isFpuEnable\(\)\)",
            r"NODE_UNLOCK_IF_HELD",
            r"csrw sstatus,\s*t1",
        ),
        ordered=True,
    ),
    Check(
        name="upstream seL4 RISC-V fastpath restore FS boundary anchor",
        path="third_party/sel4-lab/sel4test/kernel/include/arch/riscv/arch/fastpath/fastpath.h",
        patterns=(
            r"static\s+inline\s+void\s+NORETURN\s+FORCE_INLINE\s+fastpath_restore",
            r"c_exit_hook\(\)",
            r"cur_thread->tcbArch\.tcbContext\.registers",
            r"set_tcb_fs_state\(cur_thread,\s*isFpuEnable\(\)\)",
            r"NODE_UNLOCK_IF_HELD",
            r"asm volatile",
        ),
        ordered=True,
    ),
    Check(
        name="upstream seL4 signal fastpath slowpaths live FPU owners",
        path="third_party/sel4-lab/sel4test/kernel/src/fastpath/fastpath.c",
        patterns=(
            r"void\s+NORETURN\s+fastpath_signal\(word_t cptr,\s*word_t msgInfo\)",
            r"dest\s*=\s*TCB_PTR\(notification_ptr_get_ntfnQueue_head\(ntfnPtr\)\)",
            r"if\s*\(!sc\)",
            r"#if\s+defined\(ENABLE_SMP_SUPPORT\)\s*&&\s*defined\(CONFIG_HAVE_FPU\)",
            r"if\s*\(nativeThreadUsingFPU\(dest\)\)",
            r"slowpath\(SysSend\)",
        ),
        ordered=True,
    ),
    Check(
        name="local kernel has no IPC fastpath bypassing FPU restore policy",
        path="kernel/src",
        patterns=(),
        forbidden_patterns=(
            r"\bfastpath_",
            r"\bfastpath::",
            r"\bmod\s+fastpath\b",
            r"\bfastpath_call\b",
            r"\bfastpath_reply_recv\b",
            r"\bfastpath_signal\b",
        ),
    ),
    Check(
        name="upstream seL4 TCB_SetFlags FPU anchor",
        path="third_party/sel4-lab/sel4test/kernel/src/object/tcb.c",
        patterns=(
            r"static\s+void\s+invokeSetFlags",
            r"flags\s*&=\s*~clear",
            r"flags\s*\|=\s*set\s*&\s*seL4_TCBFlag_MASK",
            r"thread->tcbFlags\s*=\s*flags",
            r"flags\s*&\s*seL4_TCBFlag_fpuDisabled",
            r"fpuRelease\(thread\)",
            r"thread\s*==\s*cur_thread",
            r"lazyFPURestore\(thread\)",
            r"if\s*\(call\)",
            r"setMR\(cur_thread,\s*ipcBuffer,\s*0,\s*flags\)",
        ),
        ordered=True,
    ),
    Check(
        name="upstream seL4 TCB_SetFlags decode boundary",
        path="third_party/sel4-lab/sel4test/kernel/src/object/tcb.c",
        patterns=(
            r"static\s+exception_t\s+decodeSetFlags\(cap_t cap,\s*word_t length,\s*bool_t call,\s*word_t \*buffer\)",
            r"if\s*\(length\s*<\s*2\)",
            r"current_syscall_error\.type\s*=\s*seL4_TruncatedMessage",
            r"word_t\s+clear\s*=\s*getSyscallArg\(0,\s*buffer\)",
            r"word_t\s+set\s*=\s*getSyscallArg\(1,\s*buffer\)",
            r"setThreadState\(NODE_STATE\(ksCurThread\),\s*ThreadState_Restart\)",
            r"invokeSetFlags\(thread,\s*clear,\s*set,\s*call\)",
            r"return\s+EXCEPTION_NONE",
        ),
        ordered=True,
    ),
    Check(
        name="upstream libsel4 TCB flag ABI anchor",
        path="third_party/sel4-lab/sel4test/kernel/libsel4/include/sel4/constants.h",
        patterns=(
            r"seL4_TCBFlag_NoFlag\s*=\s*0x0",
            r"seL4_TCBFlag_fpuDisabled\s*=\s*0x1",
            r"seL4_TCBFlag_MASK\s*=\s*seL4_TCBFlag_NoFlag",
            r"\|\s*seL4_TCBFlag_fpuDisabled",
        ),
        ordered=True,
    ),
    Check(
        name="upstream libsel4 TCB_SetFlags XML contract keeps flags in first MR",
        path="third_party/sel4-lab/sel4test/kernel/libsel4/include/interfaces/object-api.xml",
        patterns=(
            r"<method\s+id=\"TCBSetFlags\"\s+name=\"SetFlags\"",
            r"Currently the only flag supported is\s+<texttt text=\"seL4_TCBFlag_fpuDisabled\"/>",
            r"The resulting TCB flags value is returned in the first message register\.",
            r"<param dir=\"out\" name=\"flags\" type=\"seL4_Word\" description=\"Returned value of the TCB flags\"/>",
        ),
        ordered=True,
    ),
    Check(
        name="upstream seL4 untyped reset clears memory before ordinary TCB creation",
        path="third_party/sel4-lab/sel4test/kernel/src/object/untyped.c",
        patterns=(
            r"static\s+exception_t\s+resetUntypedCap\(cte_t \*srcSlot\)",
            r"clearMemory\(regionBase,\s*block_size\)",
            r"clearMemory\(GET_OFFSET_FREE_PTR\(regionBase,\s*offset\),\s*chunk\)",
            r"exception_t\s+invokeUntyped_Retype",
            r"resetUntypedCap\(srcSlot\)",
            r"createNewObjects\(newType,\s*srcSlot,\s*destCNode,\s*destOffset,\s*destLength",
        ),
        ordered=True,
    ),
    Check(
        name="upstream seL4 ordinary TCB creation leaves fpuDisabled clear",
        path="third_party/sel4-lab/sel4test/kernel/src/object/objecttype.c",
        patterns=(
            r"cap_t\s+createObject\(object_t t,\s*void \*regionBase,\s*word_t userSize,\s*bool_t deviceMemory\)",
            r"case\s+seL4_TCBObject:",
            r"Setup non-zero parts of the TCB",
            r"Arch_initContext\(&tcb->tcbArch\.tcbContext\)",
            r"tcb->tcbDomain\s*=\s*ksCurDomain",
            r"return\s+cap_thread_cap_new\(TCB_REF\(tcb\)\)",
        ),
        ordered=True,
        forbidden_patterns=(
            r"case\s+seL4_TCBObject:(?:(?!case\s+seL4_EndpointObject:).)*tcbFlags",
        ),
    ),
    Check(
        name="upstream seL4 initial thread setup does not overwrite FPU state or flags",
        path="third_party/sel4-lab/sel4test/kernel/src/kernel/boot.c",
        patterns=(
            r"BOOT_CODE\s+tcb_t\s+\*create_initial_thread",
            r"tcb_t\s+\*tcb\s*=\s*TCB_PTR\(rootserver\.tcb\s+\+\s+TCB_OFFSET\)",
            r"Arch_initContext\(&tcb->tcbArch\.tcbContext\)",
            r"setRegister\(tcb,\s*capRegister,\s*bi_frame_vptr\)",
            r"setNextPC\(tcb,\s*ui_v_entry\)",
            r"setThreadState\(tcb,\s*ThreadState_Running\)",
            r"return\s+tcb;",
        ),
        ordered=True,
        forbidden_patterns=(
            r"BOOT_CODE\s+tcb_t\s+\*create_initial_thread(?:(?!\n\}).)*"
            r"(?:fpuState|tcbFlags)",
        ),
    ),
    Check(
        name="upstream RISC-V UserException fault ABI anchor",
        path="third_party/sel4-lab/sel4test/kernel/libsel4/sel4_arch_include/riscv64/sel4/sel4_arch/constants.h",
        patterns=(
            r"seL4_UserException_FaultIP",
            r"seL4_UserException_SP",
            r"seL4_UserException_Number",
            r"seL4_UserException_Code",
            r"seL4_UserException_Length",
        ),
        ordered=True,
    ),
    Check(
        name="upstream seL4 RISC-V illegal instruction fault number anchor",
        path="third_party/sel4-lab/sel4test/kernel/include/arch/riscv/arch/machine/hardware.h",
        patterns=(
            r"enum\s+vm_fault_type",
            r"RISCVInstructionIllegal\s*=\s*2",
        ),
        ordered=True,
    ),
    Check(
        name="upstream seL4 RISC-V non-VM exceptions become UserException scause zero-code faults",
        path="third_party/sel4-lab/sel4test/kernel/src/arch/riscv/c_traps.c",
        patterns=(
            r"word_t\s+scause\s*=\s*read_scause\(\)",
            r"case\s+RISCVInstructionAccessFault:",
            r"case\s+RISCVLoadAccessFault:",
            r"case\s+RISCVStoreAccessFault:",
            r"case\s+RISCVLoadPageFault:",
            r"case\s+RISCVStorePageFault:",
            r"case\s+RISCVInstructionPageFault:",
            r"handleVMFaultEvent\(scause\)",
            r"default:",
            r"handleUserLevelFault\(scause,\s*0\)",
        ),
        ordered=True,
    ),
    Check(
        name="upstream seL4 UserException stores Number and Code from user-level fault args",
        path="third_party/sel4-lab/sel4test/kernel/src/api/syscall.c",
        patterns=(
            r"exception_t\s+handleUserLevelFault\(word_t w_a,\s*word_t w_b\)",
            r"current_fault\s*=\s*seL4_Fault_UserException_new\(w_a,\s*w_b\)",
            r"handleFault\(NODE_STATE\(ksCurThread\)\)",
            r"schedule\(\)",
            r"activateThread\(\)",
        ),
        ordered=True,
    ),
    Check(
        name="upstream RISC-V UserException reply writes only FaultIP and SP",
        path="third_party/sel4-lab/sel4test/kernel/include/arch/riscv/arch/machine/registerset.h",
        patterns=(
            r"#define\s+EXCEPTION_MESSAGE",
            r"\[seL4_UserException_FaultIP\]\s*=\s*FaultIP",
            r"\[seL4_UserException_SP\]\s*=\s*SP",
            r"#define\s+SYSCALL_MESSAGE",
        ),
        ordered=True,
    ),
    Check(
        name="upstream seL4 UserException fault replies use exception message length",
        path="third_party/sel4-lab/sel4test/kernel/src/api/faults.c",
        patterns=(
            r"static\s+inline\s+void\s+copyMRsFaultReply",
            r"fault_messages\[id\]\[i\]",
            r"setRegister\(receiver,\s*r,\s*sanitiseRegister\(r,\s*v,\s*archInfo\)\)",
            r"case\s+seL4_Fault_UserException:",
            r"copyMRsFaultReply\(sender,\s*receiver,\s*MessageID_Exception,\s*MIN\(length,\s*n_exceptionMessage\)\)",
            r"return\s+\(label\s*==\s*0\)",
        ),
        ordered=True,
    ),
    Check(
        name="upstream seL4 RISC-V TCB register ABI anchor",
        path="third_party/sel4-lab/sel4test/kernel/src/arch/riscv/machine/registerset.c",
        patterns=(
            r"const\s+register_t\s+frameRegisters\[\]\s*=\s*\{",
            r"FaultIP,\s*ra,\s*sp,\s*gp",
            r"s0,\s*s1,\s*s2,\s*s3,\s*s4,\s*s5,\s*s6,\s*s7,\s*s8,\s*s9,\s*s10,\s*s11",
            r"const\s+register_t\s+gpRegisters\[\]\s*=\s*\{",
            r"a0,\s*a1,\s*a2,\s*a3,\s*a4,\s*a5,\s*a6,\s*a7",
            r"t0,\s*t1,\s*t2,\s*t3,\s*t4,\s*t5,\s*t6",
            r"tp,",
        ),
        ordered=True,
    ),
    Check(
        name="upstream libsel4 RISC-V UserContext ABI anchor",
        path="third_party/sel4-lab/sel4test/kernel/libsel4/arch_include/riscv/sel4/arch/types.h",
        patterns=(
            r"typedef\s+struct\s+seL4_UserContext_",
            r"seL4_Word\s+pc;",
            r"seL4_Word\s+ra;",
            r"seL4_Word\s+sp;",
            r"seL4_Word\s+gp;",
            r"seL4_Word\s+s0;",
            r"seL4_Word\s+s1;",
            r"seL4_Word\s+s2;",
            r"seL4_Word\s+s3;",
            r"seL4_Word\s+s4;",
            r"seL4_Word\s+s5;",
            r"seL4_Word\s+s6;",
            r"seL4_Word\s+s7;",
            r"seL4_Word\s+s8;",
            r"seL4_Word\s+s9;",
            r"seL4_Word\s+s10;",
            r"seL4_Word\s+s11;",
            r"seL4_Word\s+a0;",
            r"seL4_Word\s+a1;",
            r"seL4_Word\s+a2;",
            r"seL4_Word\s+a3;",
            r"seL4_Word\s+a4;",
            r"seL4_Word\s+a5;",
            r"seL4_Word\s+a6;",
            r"seL4_Word\s+a7;",
            r"seL4_Word\s+t0;",
            r"seL4_Word\s+t1;",
            r"seL4_Word\s+t2;",
            r"seL4_Word\s+t3;",
            r"seL4_Word\s+t4;",
            r"seL4_Word\s+t5;",
            r"seL4_Word\s+t6;",
            r"seL4_Word\s+tp;",
            r"}\s*seL4_UserContext;",
        ),
        ordered=True,
    ),
    Check(
        name="upstream seL4 RISC-V CopyRegisters has no arch FPU transfer",
        path="third_party/sel4-lab/sel4test/kernel/src/arch/riscv/object/tcb.c",
        patterns=(
            r"word_t\s+CONST\s+Arch_decodeTransfer\(word_t flags\)",
            r"return\s+0;",
            r"exception_t\s+CONST\s+Arch_performTransfer\(word_t arch,\s*tcb_t \*tcb_src,\s*tcb_t \*tcb_dest\)",
            r"return\s+EXCEPTION_NONE;",
        ),
        ordered=True,
    ),
    Check(
        name="upstream seL4 idle thread starts FPU-disabled and idle handoff does not release owner",
        path="third_party/sel4-lab/sel4test/kernel/src/kernel/thread.c",
        patterns=(
            r"BOOT_CODE\s+void\s+configureIdleThread\(tcb_t \*tcb\)",
            r"tcb->tcbFlags\s*=\s*seL4_TCBFlag_fpuDisabled",
            r"Arch_configureIdleThread\(tcb\)",
            r"setThreadState\(tcb,\s*ThreadState_IdleThreadState\)",
            r"void\s+switchToIdleThread\(void\)",
            r"Arch_switchToIdleThread\(\)",
            r"NODE_STATE\(ksCurThread\)\s*=\s*NODE_STATE\(ksIdleThread\)",
        ),
        ordered=True,
        forbidden_patterns=(
            r"void\s+switchToIdleThread\(void\)\s*\{(?:(?!\n\}).)*fpu",
        ),
    ),
    Check(
        name="upstream seL4 domain handoff FPU release anchor",
        path="third_party/sel4-lab/sel4test/kernel/src/kernel/thread.c",
        patterns=(
            r"void\s+prepareSetDomain\(tcb_t \*tptr,\s*dom_t dom\)",
            r"ksCurDomain\s*!=\s*dom",
            r"fpuRelease\(tptr\)",
            r"static\s+void\s+prepareNextDomain\(void\)",
            r"switchLocalFpuOwner\(NULL\)",
        ),
        ordered=True,
    ),
    Check(
        name="local FPU domain-handoff scope is single-domain",
        path="kernel/src/abi/constants.rs",
        patterns=(r"pub\s+const\s+NUM_DOMAINS:\s*usize\s*=\s*1;",),
    ),
    Check(
        name="local DomainSet is single-domain metadata only",
        path="kernel/src/api/invocation.rs",
        patterns=(
            r"This build has `CONFIG_NUM_DOMAINS = 1`",
            r"pub\s+fn\s+handle_domain",
            r"if\s+domain\s+>=\s+crate::abi::constants::NUM_DOMAINS\s+as\s+u64",
            r"return\s+Err\(SyscallError::InvalidArgument\)",
            r"unsafe\s+\{\s+crate::object::tcb::set_domain\(tcb_ptr,\s*domain as u8\)\s+\};",
        ),
        ordered=True,
        forbidden_patterns=(
            r"pub\s+fn\s+handle_domain(?:(?!\npub\s+fn\s+handle_sched_control).)*"
            r"(?:arch::current::fpu|arch::riscv64::fpu|fpu::release|prepare_next_domain|prepare_set_domain)",
        ),
    ),
    Check(
        name="local scheduler has no live domain rotation path",
        path="kernel/src/object/tcb.rs",
        patterns=(
            r"pub\s+fn\s+schedule\(\)\s*->\s*\*mut Tcb",
            r"pub\s+fn\s+peek_schedule\(\)\s*->\s*\*mut Tcb",
            r"fn\s+schedule_head\(\)\s*->\s*\*mut Tcb",
        ),
        ordered=True,
        forbidden_patterns=(
            r"pub\s+fn\s+schedule\(\)\s*->\s*\*mut Tcb"
            r"(?:(?!\npub unsafe fn is_runnable_on_current_core).)*\bdomain\b",
            r"\bprepare_next_domain\b",
            r"\bprepare_set_domain\b",
            r"\bks_cur_domain\b",
        ),
    ),
    Check(
        name="upstream seL4 SMP migration releases remote FPU owner anchor",
        path="third_party/sel4-lab/sel4test/kernel/src/model/smp.c",
        patterns=(
            r"void\s+migrateTCB\(tcb_t \*tcb,\s*word_t new_core\)",
            r"CONFIG_HAVE_FPU",
            r"fpuRelease\(tcb\)",
            r"tcb->tcbAffinity\s*=\s*new_core",
        ),
        ordered=True,
    ),
    Check(
        name="upstream seL4 Thread finalisation calls RISC-V FPU release hook",
        path="third_party/sel4-lab/sel4test/kernel/src/object/objecttype.c",
        patterns=(
            r"case\s+cap_thread_cap:",
            r"if\s*\(final\)",
            r"tcb\s*=\s*TCB_PTR\(cap_thread_cap_get_capTCBPtr\(cap\)\)",
            r"unbindNotification\(tcb\)",
            r"suspend\(tcb\)",
            r"Arch_prepareThreadDelete\(tcb\)",
        ),
        ordered=True,
    ),
    Check(
        name="upstream seL4 RISC-V prepareThreadDelete releases FPU owner",
        path="third_party/sel4-lab/sel4test/kernel/src/arch/riscv/object/objecttype.c",
        patterns=(
            r"void\s+Arch_prepareThreadDelete\(tcb_t \*thread\)",
            r"#ifdef\s+CONFIG_HAVE_FPU",
            r"fpuRelease\(thread\)",
            r"#endif",
        ),
        ordered=True,
    ),
    Check(
        name="upstream seL4 RISC-V remote FPU owner switch anchor",
        path="third_party/sel4-lab/sel4test/kernel/src/arch/riscv/smp/ipi.c",
        patterns=(
            r"void\s+handleRemoteCall\(IpiRemoteCall_t call,\s*word_t arg0,\s*word_t arg1,\s*word_t arg2,\s*bool_t irqPath\)",
            r"case\s+IpiRemoteCall_switchFpuOwner:",
            r"switchLocalFpuOwner\(\(tcb_t \*\)arg0\)",
            r"ipi_wait\(\)",
        ),
        ordered=True,
    ),
    Check(
        name="upstream seL4 keeps remote TCB stall separate from FPU owner switch",
        path="third_party/sel4-lab/sel4test/kernel/src/arch/riscv/smp/ipi.c",
        patterns=(
            r"case\s+IpiRemoteCall_Stall:",
            r"ipiStallCoreCallback\(irqPath\)",
            r"break;",
            r"case\s+IpiRemoteCall_switchFpuOwner:",
            r"switchLocalFpuOwner\(\(tcb_t \*\)arg0\)",
            r"break;",
        ),
        ordered=True,
        forbidden_patterns=(
            r"case\s+IpiRemoteCall_Stall:(?:(?!case\s+IpiRemoteCall_switchFpuOwner:).)*"
            r"switchLocalFpuOwner",
            r"case\s+IpiRemoteCall_Stall:(?:(?!case\s+IpiRemoteCall_switchFpuOwner:).)*"
            r"fpuRelease",
        ),
    ),
    Check(
        name="upstream seL4 RISC-V remote FPU owner request helper anchor",
        path="third_party/sel4-lab/sel4test/kernel/include/arch/riscv/arch/smp/ipi_inline.h",
        patterns=(
            r"static\s+inline\s+void\s+doRemoteswitchFpuOwner\(tcb_t \*new_owner,\s*word_t cpu\)",
            r"doRemoteOp1Arg\(IpiRemoteCall_switchFpuOwner,\s*\(word_t\)new_owner,\s*cpu\)",
        ),
        ordered=True,
    ),
    Check(
        name="upstream seL4 FPU release only clears native owners",
        path="third_party/sel4-lab/sel4test/kernel/src/machine/fpu.c",
        patterns=(
            r"void\s+fpuRelease\(tcb_t \*thread\)",
            r"nativeThreadUsingFPU\(thread\)",
            r"switchFpuOwner\(NULL,\s*SMP_TERNARY\(thread->tcbAffinity,\s*0\)\)",
        ),
        ordered=True,
    ),
    Check(
        name="upstream sel4test FPU basic operation expectation",
        path="third_party/sel4-lab/sel4test/projects/sel4test/apps/sel4test-tests/src/tests/fpu.c",
        patterns=(
            r"static\s+int\s+test_fpu_trivial\(env_t env\)",
            r"double\s+a\s*=\s*\(double\)3\.141592653589793238462643383279",
            r"for\s*\(i\s*=\s*0;\s*i\s*<\s*100;\s*i\+\+\)",
            r"a\s*=\s*a\s*\*\s*3\s*\+\s*\(a\s*/\s*3\)",
            r"b\s*=\s*a",
            r"return\s+sel4test_get_result\(\)",
            r"DEFINE_TEST\(FPU0000,\s*\"Ensure that simple FPU operations work\"",
        ),
        ordered=True,
    ),
    Check(
        name="upstream sel4test FPU multithread context-switch expectation",
        path="third_party/sel4-lab/sel4test/projects/sel4test/apps/sel4test-tests/src/tests/fpu.c",
        patterns=(
            r"static\s+int\s+test_fpu_multithreaded\(struct env \*env\)",
            r"const\s+int\s+NUM_THREADS\s*=\s*4",
            r"start_helper\(env,\s*&thread\[i\],\s*fpu_worker",
            r"num_preemptions\s*\+=\s*wait_for_helper\(&thread\[i\]\)",
            r"test_assert\(thread_state\[i\]\s*==\s*thread_state\[\(i\s*\+\s*1\)\s*%\s*NUM_THREADS\]\)",
            r"iterations\s*\*=\s*2",
            r"while\s*\(num_preemptions\s*<\s*20\)",
            r"DEFINE_TEST\(FPU0001,\s*\"Ensure multiple threads can use FPU simultaneously\"",
            r"test_fpu_multithreaded,\s*false\)",
        ),
        ordered=True,
    ),
    Check(
        name="upstream sel4test FPU migration validation scope anchor",
        path="third_party/sel4-lab/sel4test/projects/sel4test/apps/sel4test-tests/src/tests/fpu.c",
        patterns=(
            r"int\s+smp_test_fpu\(env_t env\)",
            r"volatile\s+double\s+ex\s*=\s*fpu_calculation\(\)",
            r"set_helper_affinity\(env,\s*&t\[i\],\s*i\)",
            r"start_helper\(env,\s*&t\[i\],\s*\(helper_fn_t\)\s*smp_fpu_worker",
            r"for\s*\(int it\s*=\s*0;\s*it\s*<\s*100;\s*it\+\+\)",
            r"set_helper_affinity\(env,\s*&t\[i\],\s*\(i\s*\+\s*1\)\s*%\s*env->cores\)",
            r"test_check\(wait_for_helper\(&t\[i\]\)\s*==\s*0\)",
            r"DEFINE_TEST\(FPU0002,\s*\"Test FPU remain valid across core migration\"",
            r"CONFIG_MAX_NUM_NODES\s*>\s*1",
        ),
        ordered=True,
    ),
    Check(
        name="upstream sel4test FPU TCB_SetFlags ABI expectations",
        path="third_party/sel4-lab/sel4test/projects/sel4test/apps/sel4test-tests/src/tests/fpu.c",
        patterns=(
            r"int\s+test_setflags\(env_t env\)",
            r"res\s*=\s*seL4_TCB_SetFlags\(t\.thread\.tcb\.cptr,\s*0,\s*0\)",
            r"test_check\(res\.flags\s*==\s*0\)",
            r"res\s*=\s*seL4_TCB_SetFlags\(t\.thread\.tcb\.cptr,\s*0,\s*seL4_TCBFlag_fpuDisabled\)",
            r"test_check\(res\.flags\s*==\s*seL4_TCBFlag_fpuDisabled\)",
            r"res\s*=\s*seL4_TCB_SetFlags\(t\.thread\.tcb\.cptr,\s*seL4_TCBFlag_fpuDisabled,\s*0\)",
            r"test_check\(res\.flags\s*==\s*0\)",
            r"DEFINE_TEST\(FPU0003,\s*\"Test seL4_TCB_SetFlags\"",
        ),
        ordered=True,
    ),
    Check(
        name="upstream sel4test FPU disable fault and re-enable expectations",
        path="third_party/sel4-lab/sel4test/projects/sel4test/apps/sel4test-tests/src/tests/fpu.c",
        patterns=(
            r"int\s+test_disable_enable\(env_t env\)",
            r"seL4_TCB_SetSpace\(t\.thread\.tcb\.cptr,\s*t\.local_endpoint\.cptr",
            r"start_helper\(env,\s*&t,\s*\(helper_fn_t\)fpu_worker2",
            r"res\s*=\s*seL4_TCB_SetFlags\(t\.thread\.tcb\.cptr,\s*0,\s*seL4_TCBFlag_fpuDisabled\)",
            r"test_check\(res\.flags\s*==\s*seL4_TCBFlag_fpuDisabled\)",
            r"tag\s*=\s*api_recv\(t\.local_endpoint\.cptr,\s*NULL,\s*t\.thread\.reply\.cptr\)",
            r"test_check\(seL4_MessageInfo_get_label\(tag\)\s*==\s*seL4_Fault_UserException\)",
            r"res\s*=\s*seL4_TCB_SetFlags\(t\.thread\.tcb\.cptr,\s*seL4_TCBFlag_fpuDisabled,\s*0\)",
            r"test_check\(res\.flags\s*==\s*0\)",
            r"api_reply\(t\.thread\.reply\.cptr,\s*seL4_MessageInfo_new\(0,\s*0,\s*0,\s*0\)\)",
            r"test_check\(wait_for_helper\(&t\)\s*==\s*0\)",
            r"DEFINE_TEST\(FPU0004,\s*\"Test disabling and re-enabling FPU\"",
        ),
        ordered=True,
    ),
    Check(
        name="per-core owner state starts zeroed",
        path="kernel/src/arch/riscv64/fpu.rs",
        patterns=(
            r"static\s+FPU_OWNER\s*:\s*\[AtomicUsize;\s*MAX_NUM_NODES\]\s*=\s*"
            r"\[const\s*\{\s*AtomicUsize::new\(0\)\s*\};\s*MAX_NUM_NODES\];",
        ),
    ),
    Check(
        name="per-core FPU access shadow starts disabled",
        path="kernel/src/arch/riscv64/fpu.rs",
        patterns=(
            r"static\s+FPU_ACCESS_ENABLED\s*:\s*\[AtomicBool;\s*MAX_NUM_NODES\]\s*=\s*"
            r"\[const\s*\{\s*AtomicBool::new\(false\)\s*\};\s*MAX_NUM_NODES\];",
        ),
    ),
    Check(
        name="per-core FPU access shadow mirrors seL4 isFPUEnabled",
        path="kernel/src/arch/riscv64/fpu.rs",
        patterns=(
            r"fn\s+set_fs_off\(\)",
            r"csrc sstatus,\s*\{mask\}",
            r"fn\s+set_fs_clean\(\)",
            r"csrs sstatus,\s*\{mask\}",
            r"pub\s+fn\s+disable_access\(\)",
            r"FPU_ACCESS_ENABLED\[core_index\(\)\]\.store\(false,\s*Ordering::Release\)",
            r"fn\s+enable_access\(\)",
            r"FPU_ACCESS_ENABLED\[core_index\(\)\]\.store\(true,\s*Ordering::Release\)",
            r"fn\s+access_enabled\(\)\s*->\s*bool",
            r"FPU_ACCESS_ENABLED\[core_index\(\)\]\.load\(Ordering::Acquire\)",
        ),
        ordered=True,
    ),
    Check(
        name="local FPU disable access is shadow-only like upstream disableFpu",
        path="kernel/src/arch/riscv64/fpu.rs",
        patterns=(
            r"pub\s+fn\s+disable_access\(\)",
            r"FPU_ACCESS_ENABLED\[core_index\(\)\]\.store\(false,\s*Ordering::Release\)",
        ),
        ordered=True,
        forbidden_patterns=(
            r"pub\s+fn\s+disable_access\(\)\s*\{(?:(?!\n\}).)*"
            r"(?:asm!|sstatus|set_fs_)",
        ),
    ),
    Check(
        name="local FPU enable access is shadow-only like upstream enableFpu",
        path="kernel/src/arch/riscv64/fpu.rs",
        patterns=(
            r"fn\s+enable_access\(\)",
            r"FPU_ACCESS_ENABLED\[core_index\(\)\]\.store\(true,\s*Ordering::Release\)",
        ),
        ordered=True,
        forbidden_patterns=(
            r"fn\s+enable_access\(\)\s*\{(?:(?!\n\}).)*"
            r"(?:asm!|sstatus|set_fs_)",
        ),
    ),
    Check(
        name="local FPU helpers stay within seL4-used FS off/clean states",
        path="kernel/src/arch/riscv64/fpu.rs",
        patterns=(
            r"fn\s+set_fs_off\(\)",
            r"fn\s+set_fs_clean\(\)",
        ),
        ordered=True,
        forbidden_patterns=(
            r"SSTATUS_FS_INITIAL",
            r"SSTATUS_FS_DIRTY",
            r"set_fs_initial",
            r"set_fs_dirty",
            r"read_sstatus_fs",
        ),
    ),
    Check(
        name="local RISC-V D FPU state layout matches upstream",
        path="kernel/src/arch/riscv64/trap.rs",
        patterns=(
            r"pub\s+const\s+RISCV_NUM_FP_REGS:\s*usize\s*=\s*32",
            r"pub\s+const\s+RISCV_FP_REG_BYTES:\s*usize\s*=\s*8",
            r"pub\s+const\s+RISCV_FPU_STATE_BYTES:\s*usize\s*="
            r"\s*\(RISCV_NUM_FP_REGS\s*\*\s*RISCV_FP_REG_BYTES\)\s*\+\s*8",
            r"pub\s+struct\s+FpuState",
            r"pub\s+regs:\s*\[u64;\s*RISCV_NUM_FP_REGS\]",
            r"pub\s+fcsr:\s*u32",
            r"pub\s+_pad:\s*u32",
            r"core::mem::size_of::<UserContext>\(\)\s*==\s*68\s*\*\s*8",
            r"core::mem::size_of::<FpuState>\(\)\s*==\s*RISCV_FPU_STATE_BYTES",
            r"core::mem::offset_of!\(UserContext,\s*fpu\)\s*==\s*35\s*\*\s*8",
            r"core::mem::offset_of!\(FpuState,\s*fcsr\)\s*==\s*"
            r"RISCV_NUM_FP_REGS\s*\*\s*RISCV_FP_REG_BYTES",
        ),
        ordered=True,
    ),
    Check(
        name="local initial user sstatus leaves FS for lazy FPU restore",
        path="kernel/src/arch/riscv64/trap.rs",
        patterns=(
            r"pub\s+const\s+SSTATUS_SPIE:\s*u64\s*=\s*1\s*<<\s*5",
            r"pub\s+const\s+SSTATUS_FS_MASK:\s*u64\s*=\s*0b11\s*<<\s*13",
            r"pub\s+const\s+SSTATUS_FS_CLEAN:\s*u64\s*=\s*0b10\s*<<\s*13",
            r"pub\s+const\s+USER_SSTATUS:\s*u64\s*=\s*SSTATUS_SPIE",
            r"pub\s+const\s+ROOTSERVER_SSTATUS:\s*u64\s*=\s*USER_SSTATUS\s*\|\s*SSTATUS_SUM",
        ),
        ordered=True,
        forbidden_patterns=(
            r"pub\s+const\s+USER_SSTATUS:[^\n]*SSTATUS_FS",
            r"pub\s+const\s+ROOTSERVER_SSTATUS:[^\n]*SSTATUS_FS",
        ),
    ),
    Check(
        name="local RISC-V sstatus FS bit encoding matches upstream",
        path="kernel/src/arch/riscv64/trap.rs",
        patterns=(
            r"pub\s+const\s+SSTATUS_FS_MASK:\s*u64\s*=\s*0b11\s*<<\s*13",
            r"pub\s+const\s+SSTATUS_FS_CLEAN:\s*u64\s*=\s*0b10\s*<<\s*13",
        ),
        ordered=True,
        forbidden_patterns=(
            r"SSTATUS_FS_INITIAL",
            r"SSTATUS_FS_DIRTY",
        ),
    ),
    Check(
        name="local FPU zero constructors clear the saved FP image",
        path="kernel/src/arch/riscv64/trap.rs",
        patterns=(
            r"impl\s+FpuState",
            r"pub\s+const\s+fn\s+zero\(\)\s*->\s*Self",
            r"regs:\s*\[0;\s*RISCV_NUM_FP_REGS\]",
            r"fcsr:\s*0",
            r"_pad:\s*0",
            r"impl\s+UserContext",
            r"pub\s+const\s+fn\s+zero\(\)\s*->\s*Self",
            r"fpu:\s*FpuState::zero\(\)",
        ),
        ordered=True,
    ),
    Check(
        name="local RISC-V fcsr CSR helpers match upstream",
        path="kernel/src/arch/riscv64/fpu.rs",
        patterns=(
            r"fn\s+read_fcsr\(\)\s*->\s*u32",
            r'asm!\("csrr \{0\}, fcsr"',
            r"value\s+as\s+u32",
            r"fn\s+write_fcsr\(value:\s*u32\)",
            r'asm!\("csrw fcsr, \{0\}"',
        ),
        ordered=True,
    ),
    Check(
        name="local RISC-V FPU save stores f0-f31 and fcsr",
        path="kernel/src/arch/riscv64/fpu.rs",
        patterns=(
            r"unsafe\s+fn\s+save_fpu_state\(thread:\s*\*mut Tcb\)",
            r"set_fs_clean\(\)",
            *rust_fp_reg_patterns("fsd"),
            r"dest\.fcsr\s*=\s*read_fcsr\(\)",
        ),
        ordered=True,
    ),
    Check(
        name="local RISC-V FPU load restores f0-f31 and fcsr",
        path="kernel/src/arch/riscv64/fpu.rs",
        patterns=(
            r"unsafe\s+fn\s+load_fpu_state\(thread:\s*\*const Tcb\)",
            r"set_fs_clean\(\)",
            *rust_fp_reg_patterns("fld"),
            r"write_fcsr\(src\.fcsr\)",
        ),
        ordered=True,
    ),
    Check(
        name="local FPU save/load asm keeps FPU state memory visible",
        path="kernel/src/arch/riscv64/fpu.rs",
        patterns=(
            r"unsafe\s+fn\s+save_fpu_state\(thread:\s*\*mut Tcb\)",
            r"asm!\(",
            r"options\(nostack\)",
            r"unsafe\s+fn\s+load_fpu_state\(thread:\s*\*const Tcb\)",
            r"asm!\(",
            r"options\(nostack\)",
        ),
        ordered=True,
        forbidden_patterns=(
            r"unsafe\s+fn\s+save_fpu_state\(thread:\s*\*mut Tcb\)\s*\{"
            r"(?:(?!\nunsafe\s+fn\s+load_fpu_state).)*options\([^)]*nomem",
            r"unsafe\s+fn\s+load_fpu_state\(thread:\s*\*const Tcb\)\s*\{"
            r"(?:(?!\nunsafe\s+fn\s+switch_local_owner).)*options\([^)]*nomem",
        ),
    ),
    Check(
        name="per-core FPU init matches seL4 reset shape",
        path="kernel/src/arch/riscv64/fpu.rs",
        patterns=(
            r"pub\s+fn\s+init_current_core\(\)",
            r"FPU_OWNER\[core\]\.store\(0,\s*Ordering::Release\)",
            r"set_fs_clean\(\)",
            r"write_fcsr\(0\)",
            r"disable_access\(\)",
        ),
        ordered=True,
    ),
    Check(
        name="primary rootserver boot initialises the local FPU owner",
        path="kernel/src/kernel/boot.rs",
        patterns=(
            r"pub\s+fn\s+bringup_rootserver",
            r"init_current_hart\(args\.hart_id,\s*args\.core_id\)",
            r"fpu::init_current_core\(\)",
            r"install_trap_vector\(\)",
        ),
        ordered=True,
    ),
    Check(
        name="RISC-V entry and secondary harts initialise FPU access state",
        path="kernel/src/arch/riscv64/boot.rs",
        patterns=(
            r"Clear sstatus\.FS before any hart enters Rust or parks",
            r"csrc\s+sstatus,\s*t0",
            r"pub\s+extern\s+\"C\"\s+fn\s+init_kernel",
            r"fpu::clear_supervisor_access\(\)",
            r"fpu::disable_access\(\)",
            r"pub\s+extern\s+\"C\"\s+fn\s+init_secondary_hart",
            r"fpu::init_current_core\(\)",
        ),
        ordered=True,
    ),
    Check(
        name="local owner switch spills old owner before loading new one",
        path="kernel/src/arch/riscv64/fpu.rs",
        patterns=(
            r"unsafe\s+fn\s+switch_local_owner",
            r"enable_access\(\)",
            r"save_fpu_state\(old_owner\)",
            r"disable_access\(\)",
            r"load_fpu_state\(new_owner\)",
            r"set_fpu_context_enabled\(new_owner,\s*access_enabled\(\)\)",
            r"FPU_OWNER\[core\]\.store\(new_owner as usize,\s*Ordering::Release\)",
        ),
        ordered=True,
    ),
    Check(
        name="local FPU owner lookup scans per-core owner slots",
        path="kernel/src/arch/riscv64/fpu.rs",
        patterns=(
            r"fn\s+owner_core\(thread:\s*\*const Tcb\)\s*->\s*Option<usize>",
            r"if\s+thread\.is_null\(\)",
            r"return\s+None",
            r"let\s+target\s*=\s*thread as usize",
            r"while\s+core\s*<\s*MAX_NUM_NODES",
            r"FPU_OWNER\[core\]\.load\(Ordering::Acquire\)\s*==\s*target",
            r"return\s+Some\(core\)",
            r"\n\s*None\s*\n\s*\}",
        ),
        ordered=True,
    ),
    Check(
        name="lazy restore gates disabled TCBs and reuses native owner",
        path="kernel/src/arch/riscv64/fpu.rs",
        patterns=(
            r"pub\s+fn\s+lazy_restore",
            r"fpu_disabled_snapshot\(thread\)",
            r"disable_access\(\)",
            r"set_fpu_context_enabled\(thread,\s*false\)",
            r"current_owner\(\)\s*==\s*thread",
            r"enable_access\(\)",
            r"set_fpu_context_enabled\(thread,\s*access_enabled\(\)\)",
            r"switch_local_owner\(thread\)",
        ),
        ordered=True,
    ),
    Check(
        name="local TCB FS-state helper clears then conditionally enables FS",
        path="kernel/src/object/tcb.rs",
        patterns=(
            r"pub\(crate\)\s+unsafe\s+fn\s+set_fpu_context_enabled\(tcb:\s*\*mut Tcb,\s*enabled:\s*bool\)",
            r"let\s+sstatus\s*=\s*\(\*tcb\)\.context\.sstatus\s*&\s*!SSTATUS_FS_MASK",
            r"\(\*tcb\)\.context\.sstatus\s*=\s*if\s+enabled",
            r"sstatus\s*\|\s*SSTATUS_FS_CLEAN",
            r"else",
            r"sstatus",
        ),
        ordered=True,
    ),
    Check(
        name="FPU release uses owner lookup and remote owner switch",
        path="kernel/src/arch/riscv64/fpu.rs",
        patterns=(
            r"pub\s+fn\s+release",
            r"let\s+Some\(core\)\s*=\s*owner_core\(thread\)\s+else\s*\{\s*return;\s*\}",
            r"release_on_current_core\(thread\)",
            r"remote_fpu_owner_release\(core,\s*thread\)",
        ),
        ordered=True,
    ),
    Check(
        name="local FPU release only clears current native owner",
        path="kernel/src/arch/riscv64/fpu.rs",
        patterns=(
            r"pub\s+fn\s+release\(thread:\s*\*mut Tcb\)",
            r"let\s+Some\(core\)\s*=\s*owner_core\(thread\)\s+else\s*\{\s*return;\s*\}",
            r"if\s+core\s*==\s*core_index\(\)",
            r"crate::kernel::smp::remote_fpu_owner_release\(core,\s*thread\)",
            r"pub\s+fn\s+release_on_current_core\(thread:\s*\*mut Tcb\)",
            r"if\s+thread\.is_null\(\)\s*\|\|\s*current_owner\(\)\s*!=\s*thread",
            r"unsafe\s*\{\s*switch_local_owner\(null_mut\(\)\)\s*\}",
        ),
        ordered=True,
    ),
    Check(
        name="TCB flag ABI keeps only seL4 fpuDisabled bit",
        path="kernel/src/object/tcb.rs",
        patterns=(
            r"pub\s+const\s+TCB_FLAG_FPU_DISABLED:\s*u64\s*=\s*0x1",
            r"pub\s+const\s+TCB_FLAG_MASK:\s*u64\s*=\s*TCB_FLAG_FPU_DISABLED",
        ),
    ),
    Check(
        name="local ordinary TCB allocation zeroes slab before init",
        path="kernel/src/api/invocation.rs",
        patterns=(
            r"Zero the memory we're about to repurpose \(non-device\)",
            r"ptr::write_bytes\(alloc_base_kva as \*mut u8,\s*0,\s*total_obj_bytes as usize\)",
            r"ObjectType::Tcb\s*=>\s*unsafe\s*\{\s*crate::object::tcb::init\(obj_base\)\s*\}",
        ),
        ordered=True,
    ),
    Check(
        name="local ordinary TCB flags default to seL4 NoFlag",
        path="kernel/src/object/tcb.rs",
        patterns=(
            r"pub\s+const\s+fn\s+zero\(\)\s*->\s*Self",
            r"flags:\s*0",
            r"Untyped_Retype` already zeroed the memory",
            r"pub\s+unsafe\s+fn\s+init\(tcb_kva:\s*u64\)",
            r"\(\*t\)\.state\s*=\s*ThreadState::Inactive as u8",
            r"\(\*t\)\.time_slice_ticks\s*=\s*DEFAULT_TIME_SLICE_TICKS",
            r"\(\*t\)\.context\.sstatus\s*=\s*crate::arch::current::trap::USER_SSTATUS",
        ),
        ordered=True,
        forbidden_patterns=(
            r"pub\s+unsafe\s+fn\s+init\(tcb_kva:\s*u64\)(?:(?!\n/// Detach).)*flags",
        ),
    ),
    Check(
        name="local static rootserver TCB starts from explicit zeroed FPU context",
        path="kernel/src/kernel/boot.rs",
        patterns=(
            r"struct\s+RootTcbCell\(UnsafeCell<Tcb>\)",
            r"const\s+fn\s+new\(\)\s*->\s*Self",
            r"Self\(UnsafeCell::new\(Tcb::zero\(\)\)\)",
            r"static\s+ROOTSERVER_TCB:\s*RootTcbCell\s*=\s*RootTcbCell::new\(\)",
            r"ROOTSERVER_TCB\.with_mut\(\|t\|",
            r"t\.context\.pc\s*=\s*args\.user_ventry as u64",
            r"tcb::set_current\(t\)",
            r"fpu::lazy_restore\(t\)",
        ),
        ordered=True,
    ),
    Check(
        name="sel4-user exposes TCB_SetFlags FPU flag ABI",
        path="userspace/sel4-user/src/lib.rs",
        patterns=(
            r"pub\s+const\s+LABEL_TCB_SET_FLAGS:\s*u64\s*=\s*17",
            r"pub\s+const\s+TCB_FLAG_NO_FLAG:\s*u64\s*=\s*0x0",
            r"pub\s+const\s+TCB_FLAG_FPU_DISABLED:\s*u64\s*=\s*0x1",
            r"pub\s+const\s+TCB_FLAG_MASK:\s*u64\s*=\s*TCB_FLAG_NO_FLAG\s*\|\s*TCB_FLAG_FPU_DISABLED",
            r"pub\s+struct\s+TcbSetFlagsResult",
            r"pub\s+error:\s*u64",
            r"pub\s+flags:\s*u64",
            r"pub\s+fn\s+sel4_tcb_set_flags",
            r"sel4_call",
            r"msg_info\(LABEL_TCB_SET_FLAGS,\s*0,\s*0,\s*2\)",
            r"msg_label\(reply\.info\)",
            r"reply\.mrs\[0\]",
        ),
        ordered=True,
        forbidden_patterns=(
            r"pub\s+fn\s+sel4_tcb_set_flags(?:(?!\nunsafe\s+fn\s+read_ipc_message).)*halt_loop\(\)",
        ),
    ),
    Check(
        name="TCB CopyRegisters stays limited to RISC-V frame and GP registers",
        path="kernel/src/api/invocation.rs",
        patterns=(
            r"fn\s+invoke_tcb_copy_registers",
            r"TCB_COPY_TRANSFER_FRAME",
            r"TCB_COPY_TRANSFER_INTEGER",
            r"snapshot_tcb_copy_registers\(src,\s*transfer_frame,\s*transfer_integer\)",
            r"tcb::write_user_context\(dest,\s*copied\.pc,\s*&copied\.regs\[..copied\.reg_count\]\)",
            r"fn\s+snapshot_tcb_copy_registers",
            r"if\s+transfer_frame",
            r"SEL4_TCB_FRAME_REGS\[1\.\.\]",
            r"if\s+transfer_integer",
            r"SEL4_TCB_GP_REGS",
        ),
        ordered=True,
        forbidden_patterns=(
            r"fn\s+invoke_tcb_copy_registers(?:(?!\nstruct\s+TcbCopyRegisters).)*fpu",
            r"fn\s+snapshot_tcb_copy_registers(?:(?!\n/// Verify).)*fpu",
        ),
    ),
    Check(
        name="TCB_SetFlags applies clear-then-set mask and FPU side effects",
        path="kernel/src/object/tcb.rs",
        patterns=(
            r"pub\s+unsafe\s+fn\s+set_flags",
            r"flags\s*&=\s*!clear",
            r"flags\s*\|=\s*set\s*&\s*TCB_FLAG_MASK",
            r"fpu::release\(tcb\)",
            r"set_fpu_context_enabled\(tcb,\s*false\)",
            r"current\(\)\s*==\s*tcb",
            r"fpu::lazy_restore\(tcb\)",
        ),
        ordered=True,
    ),
    Check(
        name="TCB restore boundary refreshes lazy FPU state",
        path="kernel/src/object/tcb.rs",
        patterns=(
            r"pub\(crate\)\s+unsafe\s+fn\s+prepare_for_user_restore",
            r"ThreadState::Restart",
            r"ThreadState::Running",
            r"fpu::lazy_restore\(tcb\)",
        ),
        ordered=True,
    ),
    Check(
        name="kernel exit user returns pass through the FPU restore boundary",
        path="kernel/src/arch/riscv64/trap.rs",
        patterns=(
            r"fn\s+kernel_exit\(",
            r"tcb::set_current\(next\)",
            r"let\s+ctx\s*=\s*unsafe\s*\{\s*tcb::prepare_for_user_restore\(next\)\s*\}",
            r"return\s+finish_kernel_exit\(ctx,\s*kernel_lock\)",
            r"unsafe\s*\{\s*tcb::prepare_for_user_restore\(cur\)\s*\}",
            r"return\s+finish_kernel_exit\(uc as \*mut UserContext,\s*kernel_lock\)",
            r"if\s+cur_runnable",
            r"unsafe\s*\{\s*tcb::prepare_for_user_restore\(cur\)\s*\}",
            r"return\s+finish_kernel_exit\(uc as \*mut UserContext,\s*kernel_lock\)",
            r"fn\s+kernel_exit_after_remote_stall",
            r"let\s+ctx\s*=\s*unsafe\s*\{\s*tcb::prepare_for_user_restore\(next\)\s*\}",
            r"return\s+finish_kernel_exit\(ctx,\s*kernel_lock\)",
            r"fn\s+finish_kernel_exit",
            r"kernel_lock\.defer_unlock_for_user_restore\(\)",
        ),
        ordered=True,
    ),
    Check(
        name="TCB finalisation releases live FPU owner before suspend cleanup",
        path="kernel/src/object/tcb.rs",
        patterns=(
            r"pub\s+unsafe\s+fn\s+finalize",
            r"fpu::release\(tcb\)",
            r"suspend\(tcb\)",
            r"clear_finalized_state\(tcb\)",
        ),
        ordered=True,
    ),
    Check(
        name="TCB affinity migration releases old-core FPU owner first",
        path="kernel/src/object/tcb.rs",
        patterns=(
            r"pub\s+unsafe\s+fn\s+set_affinity",
            r"core_for_affinity\(old_affinity\)\s*!=\s*core_for_affinity\(affinity\)",
            r"fpu::release\(tcb\)",
            r"\(\*tcb\)\.affinity\s*=\s*affinity",
        ),
        ordered=True,
    ),
    Check(
        name="Thread invocations stall remote current TCB before label dispatch",
        path="kernel/src/api/invocation.rs",
        patterns=(
            r"fn\s+handle_thread_inner",
            r"remote_tcb_stall\(tcb_ptr\)",
            r"match\s+label_id",
        ),
        ordered=True,
    ),
    Check(
        name="send-only Thread invocation is limited to TCB_SetFlags",
        path="kernel/src/api/invocation.rs",
        patterns=(
            r"pub\s+fn\s+handle_thread_send",
            r"InvocationLabel::TcbSetFlags",
            r"handle_thread_inner\(thread,\s*slot,\s*cap,\s*label_id,\s*length,\s*uc,\s*false\)",
        ),
        ordered=True,
    ),
    Check(
        name="local TCB_SetFlags decode boundary matches seL4",
        path="kernel/src/api/invocation.rs",
        patterns=(
            r"pub\s+fn\s+handle_thread_send",
            r"if\s+!invocation_label_matches\(label_id,\s*InvocationLabel::TcbSetFlags\)",
            r"handle_thread_inner\(thread,\s*slot,\s*cap,\s*label_id,\s*length,\s*uc,\s*false\)",
            r"id\s+if\s+id\s*==\s*InvocationLabel::TcbSetFlags\.raw\(\)\s*=>",
            r"if\s+length\s*<\s*2\s*\{\s*return\s+Err\(SyscallError::TruncatedMessage\);\s*\}",
            r"let\s+clear\s*=\s*uc\.regs\[UserRegister::A2\.index\(\)\]",
            r"let\s+set\s*=\s*uc\.regs\[UserRegister::A3\.index\(\)\]",
            r"let\s+flags\s*=\s*unsafe\s*\{\s*tcb::set_flags\(tcb_ptr,\s*clear,\s*set\)\s*\}",
            r"if\s+reply",
            r"write_reply_mr0\(uc,\s*flags\)",
        ),
        ordered=True,
    ),
    Check(
        name="TCB_SetFlags call replies return updated flags",
        path="kernel/src/api/invocation.rs",
        patterns=(
            r"InvocationLabel::TcbSetFlags",
            r"tcb::set_flags\(tcb_ptr,\s*clear,\s*set\)",
            r"if\s+reply",
            r"write_reply_mr0\(uc,\s*flags\)",
        ),
        ordered=True,
    ),
    Check(
        name="TCB_SetFlags call replies use seL4 one-word success message",
        path="kernel/src/api/invocation.rs",
        patterns=(
            r"pub\s+fn\s+success_reply_length\(tag:\s*Option<CapTag>,\s*label_id:\s*u64\)\s*->\s*u64",
            r"Some\(CapTag::Thread\)\s+if\s+label_id\s*==\s*InvocationLabel::TcbSetFlags\.raw\(\)\s*=>\s*1",
            r"fn\s+write_reply_mr0\(uc:\s*&mut UserContext,\s*value:\s*u64\)",
            r"uc\.regs\[UserRegister::A2\.index\(\)\]\s*=\s*value",
            r"write_current_ipc_buffer_word\(1,\s*value\)",
        ),
        ordered=True,
    ),
    Check(
        name="syscall success path preserves TCB_SetFlags one-word reply length",
        path="kernel/src/api/syscall.rs",
        patterns=(
            r"Ok\(\(\)\)\s*=>\s*write_ok_reply\(uc,\s*0,\s*success_reply_length\)",
            r"fn\s+write_ok_reply\(uc:\s*&mut UserContext,\s*label:\s*u64,\s*length:\s*u64\)",
            r"uc\.regs\[UserRegister::A0\.index\(\)\]\s*=\s*0",
            r"uc\.regs\[UserRegister::A1\.index\(\)\]\s*=\s*MessageInfo::new\(label,\s*0,\s*0,\s*length\)\.0",
        ),
        ordered=True,
    ),
    Check(
        name="remote FPU owner release does not deschedule target TCB",
        path="kernel/src/kernel/smp.rs",
        patterns=(
            r"REMOTE_OP_RELEASE_FPU_OWNER",
            r"let\s+op\s*=\s*REMOTE_STALL_OP\.load\(Ordering::Acquire\)",
            r"if\s+op\s*==\s*REMOTE_OP_RELEASE_FPU_OWNER",
            r"fpu::release_on_current_core\(target as \*mut Tcb\)",
            r"return\s+false",
            r"stalled_current",
        ),
        ordered=True,
    ),
    Check(
        name="remote TCB stall does not eagerly release the FPU owner",
        path="kernel/src/kernel/smp.rs",
        patterns=(
            r"fn\s+handle_remote_stall_while_waiting_for_kernel_lock\(\)\s*->\s*bool",
            r"let\s+op\s*=\s*REMOTE_STALL_OP\.load\(Ordering::Acquire\)",
            r"if\s+op\s*==\s*REMOTE_OP_RELEASE_FPU_OWNER",
            r"fpu::release_on_current_core\(target as \*mut Tcb\)",
            r"return\s+false",
            r"let\s+hart\s*=\s*current_hart\(\)",
            r"let\s+stalled_current\s*=",
        ),
        ordered=True,
        forbidden_patterns=(
            r"fn\s+handle_remote_stall_while_waiting_for_kernel_lock\(\)\s*->\s*bool\s*\{"
            r"(?:(?!if\s+op\s*==\s*REMOTE_OP_RELEASE_FPU_OWNER).)*"
            r"release_on_current_core",
        ),
    ),
    Check(
        name="no eager local owner release helper remains",
        path="kernel/src/arch/riscv64/fpu.rs",
        patterns=(),
        forbidden_patterns=(r"release_current_core_owner",),
    ),
    Check(
        name="idle handoff leaves live FPU owner for next switch or explicit release",
        path="kernel/src/kernel/smp.rs",
        patterns=(
            r"pub\s+fn\s+clear_current_state\(\)\s*\{\s*"
            r"debug_assert_kernel_lock_held\(\);\s*"
            r"let\s+hart\s+=\s+current_hart\(\);",
        ),
        forbidden_patterns=(
            r"pub\s+fn\s+clear_current_state\(\)\s*\{(?:(?!\n\}).)*fpu::",
        ),
    ),
    Check(
        name="local idle scheduler path clears current TCB without FPU release",
        path="kernel/src/arch/riscv64/trap.rs",
        patterns=(
            r"pub\s+fn\s+idle_scheduler_loop\(\)\s*->\s*!",
            r"let\s+next\s+=\s+crate::object::tcb::schedule\(\)",
            r"if\s+next\.is_null\(\)",
            r"crate::kernel::smp::clear_current_state\(\)",
            r"switch_to_kernel_vspace\(\)",
            r"None",
        ),
        ordered=True,
        forbidden_patterns=(
            r"if\s+next\.is_null\(\)\s*\{(?:(?!\n\s*\}\s*else).)*fpu::",
        ),
    ),
    Check(
        name="local idle scheduler resumes runnable TCB through FPU restore boundary",
        path="kernel/src/arch/riscv64/trap.rs",
        patterns=(
            r"pub\s+fn\s+idle_scheduler_loop\(\)\s*->\s*!",
            r"crate::object::tcb::set_current\(next\)",
            r"let\s+ctx\s*=\s*unsafe\s*\{\s*crate::object::tcb::prepare_for_user_restore\(next\)\s*\}",
            r"switch_to_tcb_vspace\(next\)",
            r"Some\(\(ctx,\s*kernel_lock\)\)",
            r"restore_user_context_locked\(ctx\)",
        ),
        ordered=True,
    ),
    Check(
        name="RISC-V trap entry keeps S-mode FS off during Rust dispatch",
        path="kernel/src/arch/riscv64/trap.S",
        patterns=(
            r"Save sepc, sstatus into UserContext\.pc / \.sstatus",
            r"li\s+t1,\s*0x6000",
            r"csrc\s+sstatus,\s*t1",
        ),
        ordered=True,
    ),
    Check(
        name="restore path writes selected TCB saved sstatus immediately before sret",
        path="kernel/src/arch/riscv64/trap.S",
        patterns=(
            r"restore_user_context:",
            r"ld\s+t2,\s*33\*8\(sp\)",
            r"csrw\s+sstatus,\s*t2",
            r"sret",
        ),
        ordered=True,
    ),
    Check(
        name="local RISC-V seL4 user-context ABI is centralized",
        path="kernel/src/arch/riscv64/trap.rs",
        patterns=(
            r"pub\s+const\s+SEL4_TCB_FRAME_REGS:\s*\[usize;\s*16\]\s*=\s*\[",
            r"0,\s*UserRegister::Ra\.index\(\),\s*UserRegister::Sp\.index\(\),\s*UserRegister::Gp\.index\(\)",
            r"8,\s*9,\s*18,\s*19,\s*20,\s*21,\s*22,\s*23,\s*24,\s*25,\s*26,\s*27",
            r"pub\s+const\s+SEL4_TCB_GP_REGS:\s*\[usize;\s*16\]\s*=\s*\[",
            r"UserRegister::A0\.index\(\),\s*UserRegister::A1\.index\(\),\s*UserRegister::A2\.index\(\),"
            r"\s*UserRegister::A3\.index\(\),\s*UserRegister::A4\.index\(\),"
            r"\s*UserRegister::A5\.index\(\),\s*UserRegister::A6\.index\(\),"
            r"\s*UserRegister::A7\.index\(\)",
            r"UserRegister::T0\.index\(\),\s*6,\s*7,\s*28,\s*29,\s*30,\s*31,\s*UserRegister::Tp\.index\(\)",
            r"pub\s+const\s+SEL4_USER_CONTEXT_WORDS:\s*usize",
            r"SEL4_TCB_FRAME_REGS\.len\(\)\s*\+\s*SEL4_TCB_GP_REGS\.len\(\)",
            r"pub\s+const\s+SEL4_USER_CONTEXT_REGS:\s*\[usize;\s*SEL4_USER_CONTEXT_WORDS\]\s*=\s*\[",
            r"0,\s*UserRegister::Ra\.index\(\),\s*UserRegister::Sp\.index\(\),\s*UserRegister::Gp\.index\(\)",
            r"8,\s*9,\s*18,\s*19,\s*20,\s*21,\s*22,\s*23,\s*24,\s*25,\s*26,\s*27",
            r"UserRegister::A0\.index\(\),\s*UserRegister::A1\.index\(\),\s*UserRegister::A2\.index\(\),"
            r"\s*UserRegister::A3\.index\(\),\s*UserRegister::A4\.index\(\),"
            r"\s*UserRegister::A5\.index\(\),\s*UserRegister::A6\.index\(\),"
            r"\s*UserRegister::A7\.index\(\)",
            r"UserRegister::T0\.index\(\),\s*6,\s*7,\s*28,\s*29,\s*30,\s*31,\s*UserRegister::Tp\.index\(\)",
        ),
        ordered=True,
    ),
    Check(
        name="TCB register invocations share centralized RISC-V seL4 ABI",
        path="kernel/src/api/invocation.rs",
        patterns=(
            r"SEL4_TCB_FRAME_REGS",
            r"SEL4_TCB_GP_REGS",
            r"SEL4_USER_CONTEXT_REGS",
            r"SEL4_USER_CONTEXT_WORDS",
            r"SEL4_USER_CONTEXT_REGS\[ctx_idx\]",
            r"count\s*>\s*SEL4_USER_CONTEXT_WORDS",
            r"SEL4_USER_CONTEXT_REGS\[i\]",
            r"SEL4_TCB_FRAME_REGS\[1\.\.\]",
            r"SEL4_TCB_GP_REGS",
        ),
        ordered=True,
        forbidden_patterns=(r"const\s+X_INDEX",),
    ),
    Check(
        name="timeout fault replies share centralized RISC-V seL4 ABI",
        path="kernel/src/api/ipc.rs",
        patterns=(
            r"SEL4_USER_CONTEXT_REGS",
            r"apply_timeout_reply",
            r"SEL4_USER_CONTEXT_REGS\[i\]",
        ),
        ordered=True,
        forbidden_patterns=(r"const\s+X_INDEX",),
    ),
    Check(
        name="sel4-user exposes RISC-V UserException fault ABI",
        path="userspace/sel4-user/src/lib.rs",
        patterns=(
            r"pub\s+const\s+FAULT_USER_EXCEPTION:\s*u64\s*=\s*3",
            r"pub\s+const\s+USER_EXCEPTION_FAULT_IP:\s*usize\s*=\s*0",
            r"pub\s+const\s+USER_EXCEPTION_SP:\s*usize\s*=\s*1",
            r"pub\s+const\s+USER_EXCEPTION_NUMBER:\s*usize\s*=\s*2",
            r"pub\s+const\s+USER_EXCEPTION_CODE:\s*usize\s*=\s*3",
            r"pub\s+const\s+USER_EXCEPTION_LENGTH:\s*usize\s*=\s*4",
        ),
        ordered=True,
    ),
    Check(
        name="RISC-V UserException fault message matches seL4 FPU-disabled trap shape",
        path="kernel/src/arch/riscv64/trap.rs",
        patterns=(
            r"IllegalInstruction\s*=\s*2",
            r"2\s*=>\s*Some\(Self::IllegalInstruction\)",
            r"send_fault_ipc\(uc,\s*code,\s*stval as u64\)",
            r"fn\s+fault_message\(code:\s*usize,\s*stval:\s*u64,\s*uc:\s*&UserContext\)",
            r"mrs\[0\]\s*=\s*uc\.pc",
            r"mrs\[1\]\s*=\s*uc\.regs\[UserRegister::Sp\.index\(\)\]",
            r"mrs\[2\]\s*=\s*code as u64",
            r"mrs\[3\]\s*=\s*0",
            r"FaultLabel::UserException\.raw\(\),\s*4,\s*mrs",
        ),
        ordered=True,
    ),
    Check(
        name="RISC-V UserException replies restore only FaultIP and SP",
        path="kernel/src/api/ipc.rs",
        patterns=(
            r"unsafe\s+fn\s+apply_user_exception_reply",
            r"let\s+n\s*=\s*\(length as usize\)\.min\(2\)",
            r"pc\s*=\s*Some\(reply_mr\(sender,\s*uc,\s*0\)\)",
            r"UserRegister::Sp\.index\(\),\s*reply_mr\(sender,\s*uc,\s*1\)",
            r"tcb::write_user_context\(caller,\s*pc,\s*&regs\[..reg_count\]\)",
        ),
        ordered=True,
    ),
    Check(
        name="rootserver first user return goes through lazy FPU restore",
        path="kernel/src/kernel/boot.rs",
        patterns=(
            r"tcb::set_current\(t\)",
            r"fpu::lazy_restore\(t\)",
            r"restore_user_context_with_kernel_lock",
        ),
        ordered=True,
    ),
)


def check_patterns(text: str, patterns: tuple[str, ...], ordered: bool) -> tuple[bool, str]:
    offset = 0
    for pattern in patterns:
        match = re.search(pattern, text[offset:], flags=re.MULTILINE | re.DOTALL)
        if match is None:
            return False, pattern
        if ordered:
            offset += match.end()
    return True, ""


def main() -> int:
    parser = argparse.ArgumentParser(
        description="Check source-level seL4 RISC-V FPU lifecycle invariants."
    )
    parser.add_argument(
        "--repo",
        type=Path,
        default=Path(__file__).resolve().parents[1],
        help="repository root (default: parent of tools/)",
    )
    parser.add_argument(
        "--verbose",
        action="store_true",
        help="print every passing check",
    )
    args = parser.parse_args()

    repo = args.repo.resolve()
    failures: list[str] = []
    passes = 0

    for check in CHECKS:
        path = repo / check.path
        if not path.exists():
            failures.append(f"{check.name}: missing {check.path}")
            continue
        text = read_check_text(path)
        ok, missing = check_patterns(text, check.patterns, check.ordered)
        if not ok:
            failures.append(f"{check.name}: missing pattern {missing!r} in {check.path}")
            continue
        forbidden_matches = [
            pattern
            for pattern in check.forbidden_patterns
            if re.search(pattern, text, flags=re.MULTILINE | re.DOTALL)
        ]
        if forbidden_matches:
            failures.append(
                f"{check.name}: forbidden pattern {forbidden_matches[0]!r} in {check.path}"
            )
            continue
        passes += 1
        if args.verbose:
            print(f"PASS: {check.name}")

    if failures:
        print(f"FAIL: {len(failures)} lifecycle checks failed", file=sys.stderr)
        for failure in failures:
            print(f"  - {failure}", file=sys.stderr)
        return 1

    print(f"PASS: {passes} seL4 FPU lifecycle source checks passed")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
