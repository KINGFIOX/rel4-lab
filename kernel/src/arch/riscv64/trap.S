// Trap entry/exit assembly for RV64 S-mode.
//
// Calling convention: kernel uses sscratch as a per-hart save slot.
//   - When in user mode, sscratch = pointer to current TCB's saved-regs area
//     (UserContext). The trap entry swaps sp/sscratch so it can spill x1..x31
//     into the TCB without clobbering sp.
//   - When in kernel mode (e.g. during boot before any user thread runs),
//     sscratch = 0. In that case the trap is a kernel-mode trap and we just
//     redirect to kernel_trap_panic with the original sp.
//
// Both paths converge on handle_trap_rust(uc: &mut UserContext).

    .section .text.traps
    .align 4
    .globl trap_entry
trap_entry:
    // Atomically swap sp <-> sscratch.
    //   If we came from user-mode: sp now points at UserContext,
    //   sscratch holds the old user sp.
    //   If we came from kernel-mode: sscratch was 0, so sp becomes 0,
    //   and sscratch holds the old kernel sp. We detect that and revert.
    csrrw   sp, sscratch, sp
    bnez    sp, 1f                  // sp != 0 -> from user, proceed

    // From-kernel path: restore kernel sp from sscratch (atomic swap put
    // the old kernel sp there). Then jump to a panic for now.
    csrrw   sp, sscratch, sp        // swap back: sp = kernel sp, sscratch = 0
    j       kernel_trap_panic

1:
    // User-mode trap. sp = UserContext base, sscratch = user sp.
    // Save x1, x3..x31 into UserContext.regs[1..32]. x0 is zero; x2 (sp)
    // is special and pulled from sscratch below.
    sd      x1,  1*8(sp)
    sd      x3,  3*8(sp)
    sd      x4,  4*8(sp)
    sd      x5,  5*8(sp)
    sd      x6,  6*8(sp)
    sd      x7,  7*8(sp)
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

    // Stash user sp (currently in sscratch) into regs[2].
    csrr    t0, sscratch
    sd      t0, 2*8(sp)
    // Now clear sscratch so a nested trap takes the kernel path.
    csrw    sscratch, x0

    // Save sepc, sstatus into UserContext.pc / .sstatus.
    csrr    t0, sepc
    sd      t0, 32*8(sp)
    csrr    t0, sstatus
    sd      t0, 33*8(sp)

    // a0 = pointer to UserContext (== current sp).
    mv      a0, sp

    // Swap to the kernel stack. Single kernel stack for now; see linker.ld.
    la      sp, __stack_top

    call    handle_trap_rust

    // handle_trap_rust returns the UserContext pointer of the TCB to
    // resume in a0 — usually the same TCB that trapped, but the
    // scheduler may have swapped to a different one. restore_user_context
    // takes a0 as its argument.
    j       restore_user_context

    .globl restore_user_context
restore_user_context:
    // a0 = pointer to UserContext to restore.
    mv      sp, a0

    // Load sepc, sstatus first.
    ld      t0, 32*8(sp)
    csrw    sepc, t0
    ld      t0, 33*8(sp)
    csrw    sstatus, t0

    // Put the UserContext pointer into sscratch so the next trap entry
    // can find it again.
    csrw    sscratch, sp

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
