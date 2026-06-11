// Trap entry/exit assembly for RV64 S-mode.
//
// Calling convention: sscratch always points at the current hart's
// TrapScratch while the kernel is initialised.
//   - TrapScratch.user_context is the current TCB's UserContext pointer.
//   - TrapScratch.kernel_stack_top is this hart's kernel stack top.
//   - Trap entry uses scratch spill slots for the user registers that must be
//     borrowed before UserContext is addressable.
//
// Both paths converge on handle_trap_rust(uc: *mut UserContext).

    .section .text.traps
    .align 4

    .equ TRAP_SCRATCH_KERNEL_STACK_TOP, 0
    .equ TRAP_SCRATCH_USER_CONTEXT, 8
    .equ TRAP_SCRATCH_SAVED_USER_SP, 16
    .equ TRAP_SCRATCH_SAVED_USER_T1, 24
    .equ TRAP_SCRATCH_SAVED_USER_T2, 32
    .equ SSTATUS_SPP, 0x100

    .globl trap_entry
trap_entry:
    // Swap t0 with sscratch so t0 becomes the TrapScratch pointer while the
    // original user/kernel t0 is parked in sscratch.
    csrrw   t0, sscratch, t0
    beqz    t0, kernel_trap_panic

    // Save the user/kernel registers borrowed by the trampoline before using
    // them as temporaries. If this was a kernel-mode trap, these writes are
    // harmless because we panic below with the original kernel sp intact.
    sd      sp, TRAP_SCRATCH_SAVED_USER_SP(t0)
    sd      t1, TRAP_SCRATCH_SAVED_USER_T1(t0)
    sd      t2, TRAP_SCRATCH_SAVED_USER_T2(t0)

    // sstatus.SPP records the privilege level before the trap.
    csrr    t1, sstatus
    li      t2, SSTATUS_SPP
    and     t1, t1, t2
    beqz    t1, 1f

    // From-kernel path: restore sscratch to the TrapScratch pointer and panic
    // on the current kernel stack.
    csrw    sscratch, t0
    j       kernel_trap_panic

1:
    // User-mode trap. Load the current TCB's UserContext.
    ld      sp, TRAP_SCRATCH_USER_CONTEXT(t0)
    bnez    sp, 2f
    csrw    sscratch, t0
    j       kernel_trap_panic

2:
    // Save x1, x3..x31 into UserContext.regs[1..32]. x0 is zero; x2 (sp)
    // and x5..x7 are pulled from the scratch slots below.
    sd      x1,  1*8(sp)
    sd      x3,  3*8(sp)
    sd      x4,  4*8(sp)
    sd      x8,  8*8(sp)
    sd      x9,  9*8(sp)
    sd      x10, 10*8(sp)
    sd      x11, 11*8(sp)
    sd      x12, 12*8(sp)
    sd      x13, 13*8(sp)
    sd      x14, 14*8(sp)
    sd      x15, 15*8(sp)
    sd      x16, 16*8(sp)
    sd      x17, 17*8(sp)
    sd      x18, 18*8(sp)
    sd      x19, 19*8(sp)
    sd      x20, 20*8(sp)
    sd      x21, 21*8(sp)
    sd      x22, 22*8(sp)
    sd      x23, 23*8(sp)
    sd      x24, 24*8(sp)
    sd      x25, 25*8(sp)
    sd      x26, 26*8(sp)
    sd      x27, 27*8(sp)
    sd      x28, 28*8(sp)
    sd      x29, 29*8(sp)
    sd      x30, 30*8(sp)
    sd      x31, 31*8(sp)

    // Fill the registers that the trampoline borrowed before UserContext was
    // addressable.
    ld      t1, TRAP_SCRATCH_SAVED_USER_SP(t0)
    sd      t1, 2*8(sp)
    csrr    t1, sscratch
    sd      t1, 5*8(sp)
    ld      t1, TRAP_SCRATCH_SAVED_USER_T1(t0)
    sd      t1, 6*8(sp)
    ld      t1, TRAP_SCRATCH_SAVED_USER_T2(t0)
    sd      t1, 7*8(sp)

    // Restore sscratch to the TrapScratch pointer so Rust can recover the
    // current hart id, and so nested S-mode traps are classified by SPP.
    csrw    sscratch, t0

    // Save sepc, sstatus into UserContext.pc / .sstatus.
    csrr    t1, sepc
    sd      t1, 32*8(sp)
    csrr    t1, sstatus
    sd      t1, 33*8(sp)
    // Keep S-mode itself with FS=Off while the Rust dispatcher runs.
    li      t1, 0x6000
    csrc    sstatus, t1

    // a0 = pointer to UserContext (== current sp).
    mv      a0, sp

    // Swap to this hart's kernel stack.
    csrr    t0, sscratch
    ld      sp, TRAP_SCRATCH_KERNEL_STACK_TOP(t0)

    call    handle_trap_rust

    // handle_trap_rust returns the UserContext pointer of the TCB to
    // resume in a0 — usually the same TCB that trapped, but the
    // scheduler may have swapped to a different one. restore_user_context
    // takes a0 as its argument.
    j       restore_user_context_locked

    .globl restore_user_context_locked
restore_user_context_locked:
    // a0 = pointer to UserContext to restore. Keep it across the Rust
    // unlock helper call while we are still on this hart's kernel stack.
    addi    sp, sp, -16
    sd      a0, 0(sp)
    call    kernel_unlock_for_user_restore
    ld      a0, 0(sp)
    addi    sp, sp, 16
    j       restore_user_context

    .globl restore_user_context
restore_user_context:
    // a0 = pointer to UserContext to restore.
    mv      sp, a0
    csrr    t0, sscratch
    bnez    t0, 1f
    j       kernel_trap_panic
1:
    sd      sp, TRAP_SCRATCH_USER_CONTEXT(t0)

    // Load sepc and sstatus. The kernel intentionally does not save or
    // restore floating-point state; force sstatus.FS=Off on every return.
    ld      t0, 32*8(sp)
    csrw    sepc, t0
    ld      t2, 33*8(sp)
    li      t3, 0x6000
    not     t3, t3
    and     t2, t2, t3
    csrw    sstatus, t2

    // Restore registers x1, x3..x31. x2 (user sp) is loaded last.
    ld      x1,  1*8(sp)
    ld      x3,  3*8(sp)
    ld      x4,  4*8(sp)
    ld      x5,  5*8(sp)
    ld      x6,  6*8(sp)
    ld      x7,  7*8(sp)
    ld      x8,  8*8(sp)
    ld      x9,  9*8(sp)
    ld      x10, 10*8(sp)
    ld      x11, 11*8(sp)
    ld      x12, 12*8(sp)
    ld      x13, 13*8(sp)
    ld      x14, 14*8(sp)
    ld      x15, 15*8(sp)
    ld      x16, 16*8(sp)
    ld      x17, 17*8(sp)
    ld      x18, 18*8(sp)
    ld      x19, 19*8(sp)
    ld      x20, 20*8(sp)
    ld      x21, 21*8(sp)
    ld      x22, 22*8(sp)
    ld      x23, 23*8(sp)
    ld      x24, 24*8(sp)
    ld      x25, 25*8(sp)
    ld      x26, 26*8(sp)
    ld      x27, 27*8(sp)
    ld      x28, 28*8(sp)
    ld      x29, 29*8(sp)
    ld      x30, 30*8(sp)
    ld      x31, 31*8(sp)
    ld      x2,  2*8(sp)            // x2 = user sp, last

    sret
