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

    // Enable S-mode access to FPRs while saving them. The trapped user
    // context may have had FS=Off (e.g. fpuDisabled tests), and an fsd
    // with FS=Off would itself fault in the kernel.
    li      t1, 0x6000             // sstatus.FS = Dirty
    csrs    sstatus, t1

    // Save floating-point state. The kernel runs user contexts with
    // sstatus.FS=Dirty, so these instructions are legal in S-mode.
    fsd     f0,  35*8(sp)
    fsd     f1,  36*8(sp)
    fsd     f2,  37*8(sp)
    fsd     f3,  38*8(sp)
    fsd     f4,  39*8(sp)
    fsd     f5,  40*8(sp)
    fsd     f6,  41*8(sp)
    fsd     f7,  42*8(sp)
    fsd     f8,  43*8(sp)
    fsd     f9,  44*8(sp)
    fsd     f10, 45*8(sp)
    fsd     f11, 46*8(sp)
    fsd     f12, 47*8(sp)
    fsd     f13, 48*8(sp)
    fsd     f14, 49*8(sp)
    fsd     f15, 50*8(sp)
    fsd     f16, 51*8(sp)
    fsd     f17, 52*8(sp)
    fsd     f18, 53*8(sp)
    fsd     f19, 54*8(sp)
    fsd     f20, 55*8(sp)
    fsd     f21, 56*8(sp)
    fsd     f22, 57*8(sp)
    fsd     f23, 58*8(sp)
    fsd     f24, 59*8(sp)
    fsd     f25, 60*8(sp)
    fsd     f26, 61*8(sp)
    fsd     f27, 62*8(sp)
    fsd     f28, 63*8(sp)
    fsd     f29, 64*8(sp)
    fsd     f30, 65*8(sp)
    fsd     f31, 66*8(sp)
    frcsr   t0
    sd      t0, 67*8(sp)

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

    // Load sepc and temporarily enable FPR access for restore. The
    // final user sstatus is written after fld/fscsr so FS=Off contexts
    // can still have their saved FPR image restored.
    ld      t0, 32*8(sp)
    csrw    sepc, t0
    ld      t2, 33*8(sp)
    li      t1, 0x6000             // sstatus.FS = Dirty
    csrs    sstatus, t1

    // Restore floating-point state before returning to user.
    fld     f0,  35*8(sp)
    fld     f1,  36*8(sp)
    fld     f2,  37*8(sp)
    fld     f3,  38*8(sp)
    fld     f4,  39*8(sp)
    fld     f5,  40*8(sp)
    fld     f6,  41*8(sp)
    fld     f7,  42*8(sp)
    fld     f8,  43*8(sp)
    fld     f9,  44*8(sp)
    fld     f10, 45*8(sp)
    fld     f11, 46*8(sp)
    fld     f12, 47*8(sp)
    fld     f13, 48*8(sp)
    fld     f14, 49*8(sp)
    fld     f15, 50*8(sp)
    fld     f16, 51*8(sp)
    fld     f17, 52*8(sp)
    fld     f18, 53*8(sp)
    fld     f19, 54*8(sp)
    fld     f20, 55*8(sp)
    fld     f21, 56*8(sp)
    fld     f22, 57*8(sp)
    fld     f23, 58*8(sp)
    fld     f24, 59*8(sp)
    fld     f25, 60*8(sp)
    fld     f26, 61*8(sp)
    fld     f27, 62*8(sp)
    fld     f28, 63*8(sp)
    fld     f29, 64*8(sp)
    fld     f30, 65*8(sp)
    fld     f31, 66*8(sp)
    ld      t0, 67*8(sp)
    fscsr   t0
    csrw    sstatus, t2

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
