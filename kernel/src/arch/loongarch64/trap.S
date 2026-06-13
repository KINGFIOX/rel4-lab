    .section .text.traps
    .align 6

    .equ TRAP_SCRATCH_KERNEL_STACK_TOP, 0
    .equ TRAP_SCRATCH_USER_CONTEXT, 8
    .equ TRAP_SCRATCH_SAVED_USER_SP, 16
    .equ TRAP_SCRATCH_SAVED_USER_T1, 24
    .equ TRAP_SCRATCH_SAVED_USER_T2, 32

    .globl trap_entry
trap_entry:
    csrwr   $t0, {csr_ks0}
    beqz    $t0, {kernel_trap_panic}

    st.d    $sp, $t0, TRAP_SCRATCH_SAVED_USER_SP
    st.d    $t1, $t0, TRAP_SCRATCH_SAVED_USER_T1
    st.d    $t2, $t0, TRAP_SCRATCH_SAVED_USER_T2

    csrrd   $t1, {csr_prmd}
    andi    $t1, $t1, {prmd_pplv_mask}
    li.w    $t2, {prmd_pplv_user}
    beq     $t1, $t2, 1f
    csrwr   $t0, {csr_ks0}
    b       {kernel_trap_panic}

1:
    ld.d    $sp, $t0, TRAP_SCRATCH_USER_CONTEXT
    bnez    $sp, 2f
    csrwr   $t0, {csr_ks0}
    b       {kernel_trap_panic}

2:
    st.d    $zero, $sp,  0*8
    st.d    $ra,   $sp,  1*8
    st.d    $tp,   $sp,  2*8
    st.d    $a0,   $sp,  4*8
    st.d    $a1,   $sp,  5*8
    st.d    $a2,   $sp,  6*8
    st.d    $a3,   $sp,  7*8
    st.d    $a4,   $sp,  8*8
    st.d    $a5,   $sp,  9*8
    st.d    $a6,   $sp, 10*8
    st.d    $a7,   $sp, 11*8
    st.d    $t3,   $sp, 15*8
    st.d    $t4,   $sp, 16*8
    st.d    $t5,   $sp, 17*8
    st.d    $t6,   $sp, 18*8
    st.d    $t7,   $sp, 19*8
    st.d    $t8,   $sp, 20*8
    st.d    $r21,  $sp, 21*8
    st.d    $fp,   $sp, 22*8
    st.d    $s0,   $sp, 23*8
    st.d    $s1,   $sp, 24*8
    st.d    $s2,   $sp, 25*8
    st.d    $s3,   $sp, 26*8
    st.d    $s4,   $sp, 27*8
    st.d    $s5,   $sp, 28*8
    st.d    $s6,   $sp, 29*8
    st.d    $s7,   $sp, 30*8
    st.d    $s8,   $sp, 31*8

    ld.d    $t1, $t0, TRAP_SCRATCH_SAVED_USER_SP
    st.d    $t1, $sp,  3*8
    csrrd   $t1, {csr_ks0}
    st.d    $t1, $sp, 12*8
    ld.d    $t1, $t0, TRAP_SCRATCH_SAVED_USER_T1
    st.d    $t1, $sp, 13*8
    ld.d    $t1, $t0, TRAP_SCRATCH_SAVED_USER_T2
    st.d    $t1, $sp, 14*8

    csrrd   $t1, {csr_era}
    st.d    $t1, $sp, 32*8
    st.d    $t1, $sp, 34*8
    csrrd   $t1, {csr_prmd}
    st.d    $t1, $sp, 33*8

    la.local $t1, {trap_record}
    csrrd   $t2, {csr_era}
    st.d    $t2, $t1, 0*8
    csrrd   $t2, {csr_prmd}
    st.d    $t2, $t1, 1*8
    csrrd   $t2, {csr_estat}
    st.d    $t2, $t1, 2*8
    csrrd   $t2, {csr_badv}
    st.d    $t2, $t1, 3*8

    ld.d    $t2, $t0, TRAP_SCRATCH_KERNEL_STACK_TOP
    csrwr   $t0, {csr_ks0}
    move    $a0, $sp
    move    $sp, $t2
    bl      {handle_trap_rust}

    b       restore_user_context_locked

    .globl restore_user_context_locked
restore_user_context_locked:
    addi.d  $sp, $sp, -16
    st.d    $a0, $sp, 0
    bl      kernel_unlock_for_user_restore
    ld.d    $a0, $sp, 0
    addi.d  $sp, $sp, 16
    b       restore_user_context

    .globl restore_user_context
restore_user_context:
    move    $t0, $a0

    csrrd   $t1, {csr_ks0}
    beqz    $t1, {kernel_trap_panic}
    st.d    $t0, $t1, TRAP_SCRATCH_USER_CONTEXT

    ld.d    $t1, $t0, 32*8
    csrwr   $t1, {csr_era}
    ld.d    $t1, $t0, 33*8
    csrwr   $t1, {csr_prmd}

    ld.d    $ra, $t0,  1*8
    ld.d    $tp, $t0,  2*8
    ld.d    $sp, $t0,  3*8
    ld.d    $a0, $t0,  4*8
    ld.d    $a1, $t0,  5*8
    ld.d    $a2, $t0,  6*8
    ld.d    $a3, $t0,  7*8
    ld.d    $a4, $t0,  8*8
    ld.d    $a5, $t0,  9*8
    ld.d    $a6, $t0, 10*8
    ld.d    $a7, $t0, 11*8
    ld.d    $t1, $t0, 13*8
    ld.d    $t2, $t0, 14*8
    ld.d    $t3, $t0, 15*8
    ld.d    $t4, $t0, 16*8
    ld.d    $t5, $t0, 17*8
    ld.d    $t6, $t0, 18*8
    ld.d    $t7, $t0, 19*8
    ld.d    $t8, $t0, 20*8
    ld.d    $r21, $t0, 21*8
    ld.d    $fp, $t0, 22*8
    ld.d    $s0, $t0, 23*8
    ld.d    $s1, $t0, 24*8
    ld.d    $s2, $t0, 25*8
    ld.d    $s3, $t0, 26*8
    ld.d    $s4, $t0, 27*8
    ld.d    $s5, $t0, 28*8
    ld.d    $s6, $t0, 29*8
    ld.d    $s7, $t0, 30*8
    ld.d    $s8, $t0, 31*8
    ld.d    $t0, $t0, 12*8

    ertn
