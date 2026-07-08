/*
 * Copyright 2020, Data61, CSIRO (ABN 41 687 119 230)
 *
 * SPDX-License-Identifier: GPL-2.0-only
 */

#pragma once

#include <util.h>
#include <arch/machine/registerset.h>
#include <arch/machine/hardware.h>
#include <arch/smp/ipi_inline.h>

static inline void set_fs_off(void)
{
    word_t euen;
    word_t mask = ~0x7ul;
    asm volatile(
        "csrrd %0, 0x2\n\t"
        "and   %0, %0, %1\n\t"
        "csrwr %0, 0x2\n\t"
        "dbar  0"
        : "=&r"(euen)
        : "r"(mask)
        : "memory"
    );
}

#ifdef CONFIG_HAVE_FPU
#define FL "fld.d"
#define FS "fst.d"
#define FP_REG_BYTES "8"

extern bool_t isFPUEnabled[CONFIG_MAX_NUM_NODES];

static inline void set_fs_clean(void)
{
    word_t euen;
    word_t mask = ~0x6ul;
    asm volatile(
        "csrrd %0, 0x2\n\t"
        "ori   %0, %0, 1\n\t"
        "and   %0, %0, %1\n\t"
        "csrwr %0, 0x2\n\t"
        "dbar  0"
        : "=&r"(euen)
        : "r"(mask)
        : "memory"
    );
}

static inline void set_fs_initial(void)
{
    set_fs_clean();
}

static inline void set_fs_dirty(void)
{
    set_fs_clean();
}

static inline word_t read_sstatus_fs(void)
{
    word_t euen;
    asm volatile("csrrd %0, 0x2" : "=r"(euen));
    return euen & 1;
}

/* We unconditionally enable FPU accesses in kernel
 * mode for save and store functions. The field
 * we be set again before returning to user-mode
 * to actually enable/disable FPU accesses in
 * user mode.
 */
static inline void saveFpuState(tcb_t *thread)
{
    user_fpu_state_t *dest = &thread->tcbArch.tcbContext.fpuState;

    set_fs_clean();

    asm volatile(
        FS " $f0,  %0, 0*"  FP_REG_BYTES "\n\t"
        FS " $f1,  %0, 1*"  FP_REG_BYTES "\n\t"
        FS " $f2,  %0, 2*"  FP_REG_BYTES "\n\t"
        FS " $f3,  %0, 3*"  FP_REG_BYTES "\n\t"
        FS " $f4,  %0, 4*"  FP_REG_BYTES "\n\t"
        FS " $f5,  %0, 5*"  FP_REG_BYTES "\n\t"
        FS " $f6,  %0, 6*"  FP_REG_BYTES "\n\t"
        FS " $f7,  %0, 7*"  FP_REG_BYTES "\n\t"
        FS " $f8,  %0, 8*"  FP_REG_BYTES "\n\t"
        FS " $f9,  %0, 9*"  FP_REG_BYTES "\n\t"
        FS " $f10, %0, 10*" FP_REG_BYTES "\n\t"
        FS " $f11, %0, 11*" FP_REG_BYTES "\n\t"
        FS " $f12, %0, 12*" FP_REG_BYTES "\n\t"
        FS " $f13, %0, 13*" FP_REG_BYTES "\n\t"
        FS " $f14, %0, 14*" FP_REG_BYTES "\n\t"
        FS " $f15, %0, 15*" FP_REG_BYTES "\n\t"
        FS " $f16, %0, 16*" FP_REG_BYTES "\n\t"
        FS " $f17, %0, 17*" FP_REG_BYTES "\n\t"
        FS " $f18, %0, 18*" FP_REG_BYTES "\n\t"
        FS " $f19, %0, 19*" FP_REG_BYTES "\n\t"
        FS " $f20, %0, 20*" FP_REG_BYTES "\n\t"
        FS " $f21, %0, 21*" FP_REG_BYTES "\n\t"
        FS " $f22, %0, 22*" FP_REG_BYTES "\n\t"
        FS " $f23, %0, 23*" FP_REG_BYTES "\n\t"
        FS " $f24, %0, 24*" FP_REG_BYTES "\n\t"
        FS " $f25, %0, 25*" FP_REG_BYTES "\n\t"
        FS " $f26, %0, 26*" FP_REG_BYTES "\n\t"
        FS " $f27, %0, 27*" FP_REG_BYTES "\n\t"
        FS " $f28, %0, 28*" FP_REG_BYTES "\n\t"
        FS " $f29, %0, 29*" FP_REG_BYTES "\n\t"
        FS " $f30, %0, 30*" FP_REG_BYTES "\n\t"
        FS " $f31, %0, 31*" FP_REG_BYTES "\n\t"
        :
        : "r"(&dest->regs[0])
        : "memory"
    );

    dest->fcsr = read_fcsr();
}

static inline void loadFpuState(const tcb_t *thread)
{
    const user_fpu_state_t *src = &thread->tcbArch.tcbContext.fpuState;

    set_fs_clean();

    asm volatile(
        FL " $f0,  %0, 0*"  FP_REG_BYTES "\n\t"
        FL " $f1,  %0, 1*"  FP_REG_BYTES "\n\t"
        FL " $f2,  %0, 2*"  FP_REG_BYTES "\n\t"
        FL " $f3,  %0, 3*"  FP_REG_BYTES "\n\t"
        FL " $f4,  %0, 4*"  FP_REG_BYTES "\n\t"
        FL " $f5,  %0, 5*"  FP_REG_BYTES "\n\t"
        FL " $f6,  %0, 6*"  FP_REG_BYTES "\n\t"
        FL " $f7,  %0, 7*"  FP_REG_BYTES "\n\t"
        FL " $f8,  %0, 8*"  FP_REG_BYTES "\n\t"
        FL " $f9,  %0, 9*"  FP_REG_BYTES "\n\t"
        FL " $f10, %0, 10*" FP_REG_BYTES "\n\t"
        FL " $f11, %0, 11*" FP_REG_BYTES "\n\t"
        FL " $f12, %0, 12*" FP_REG_BYTES "\n\t"
        FL " $f13, %0, 13*" FP_REG_BYTES "\n\t"
        FL " $f14, %0, 14*" FP_REG_BYTES "\n\t"
        FL " $f15, %0, 15*" FP_REG_BYTES "\n\t"
        FL " $f16, %0, 16*" FP_REG_BYTES "\n\t"
        FL " $f17, %0, 17*" FP_REG_BYTES "\n\t"
        FL " $f18, %0, 18*" FP_REG_BYTES "\n\t"
        FL " $f19, %0, 19*" FP_REG_BYTES "\n\t"
        FL " $f20, %0, 20*" FP_REG_BYTES "\n\t"
        FL " $f21, %0, 21*" FP_REG_BYTES "\n\t"
        FL " $f22, %0, 22*" FP_REG_BYTES "\n\t"
        FL " $f23, %0, 23*" FP_REG_BYTES "\n\t"
        FL " $f24, %0, 24*" FP_REG_BYTES "\n\t"
        FL " $f25, %0, 25*" FP_REG_BYTES "\n\t"
        FL " $f26, %0, 26*" FP_REG_BYTES "\n\t"
        FL " $f27, %0, 27*" FP_REG_BYTES "\n\t"
        FL " $f28, %0, 28*" FP_REG_BYTES "\n\t"
        FL " $f29, %0, 29*" FP_REG_BYTES "\n\t"
        FL " $f30, %0, 30*" FP_REG_BYTES "\n\t"
        FL " $f31, %0, 31*" FP_REG_BYTES "\n\t"
        :
        : "r"(&src->regs[0])
        : "memory"
    );

    write_fcsr(src->fcsr);
}

/** MODIFIES: phantom_machine_state */
/** DONT_TRANSLATE */
static inline void enableFpu(void)
{
    isFPUEnabled[CURRENT_CPU_INDEX()] = true;
}

static inline void disableFpu(void)
{
    isFPUEnabled[CURRENT_CPU_INDEX()] = false;
}

static inline bool_t isFpuEnable(void)
{
    return isFPUEnabled[CURRENT_CPU_INDEX()];
}

static inline void set_tcb_fs_state(tcb_t *tcb, bool_t enabled)
{
    (void)tcb;
    (void)enabled;
}

#endif /* end of CONFIG_HAVE_FPU */
