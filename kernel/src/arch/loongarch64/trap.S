    .section .text.traps
    .align 6

    .equ TRAP_SCRATCH_KERNEL_STACK_TOP, 0
    .equ TRAP_SCRATCH_USER_CONTEXT, 8
    .equ TRAP_SCRATCH_SAVED_USER_SP, 16
    .equ TRAP_SCRATCH_SAVED_USER_T1, 24
    .equ TRAP_SCRATCH_SAVED_USER_T2, 32
    .equ TRAP_SCRATCH_CORE_ID, 40
    .equ TRAP_SCRATCH_HART_ID, 48
    .equ USER_CONTEXT_PC, 32*8
    .equ USER_CONTEXT_SSTATUS, 33*8
    .equ USER_CONTEXT_RESTART_PC, 34*8
    .equ USER_CONTEXT_FPU, 35*8
    .equ USER_CONTEXT_TRAP_RECORD, 69*8
    .equ USER_CONTEXT_TRAP_RECORD_ERA, USER_CONTEXT_TRAP_RECORD + 0*8
    .equ USER_CONTEXT_TRAP_RECORD_PRMD, USER_CONTEXT_TRAP_RECORD + 1*8
    .equ USER_CONTEXT_TRAP_RECORD_ESTAT, USER_CONTEXT_TRAP_RECORD + 2*8
    .equ USER_CONTEXT_TRAP_RECORD_BADV, USER_CONTEXT_TRAP_RECORD + 3*8
    .equ CSR_PRMD, 0x001
    .equ CSR_ESTAT, 0x005
    .equ CSR_ERA, 0x006
    .equ CSR_BADV, 0x007
    .equ CSR_KS0, 0x030
    .equ CSR_TLBRBADV, 0x089
    .equ CSR_TLBRERA, 0x08a
    .equ CSR_TLBRSAVE, 0x08b
    .equ CSR_TLBRPRMD, 0x08f
    .equ PRMD_PPLV_MASK, 0x3
    .equ PRMD_PPLV_USER, 0x3
    .equ USER_SSTATUS, 0x7
    .equ TLBR_FALLBACK_ESTAT_PIS, 0x20000

    .globl trap_entry
trap_entry:
    csrwr   $t0, CSR_KS0
    beqz    $t0, kernel_trap_panic

    st.d    $sp, $t0, TRAP_SCRATCH_SAVED_USER_SP
    st.d    $t1, $t0, TRAP_SCRATCH_SAVED_USER_T1
    st.d    $t2, $t0, TRAP_SCRATCH_SAVED_USER_T2

    csrrd   $t1, CSR_PRMD
    andi    $t1, $t1, PRMD_PPLV_MASK
    li.w    $t2, PRMD_PPLV_USER
    beq     $t1, $t2, 1f
    csrwr   $t0, CSR_KS0
    b       kernel_trap_panic

1:
    ld.d    $sp, $t0, TRAP_SCRATCH_USER_CONTEXT
    bnez    $sp, 2f
    csrwr   $t0, CSR_KS0
    b       kernel_trap_panic

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
    csrrd   $t1, CSR_KS0
    st.d    $t1, $sp, 12*8
    ld.d    $t1, $t0, TRAP_SCRATCH_SAVED_USER_T1
    st.d    $t1, $sp, 13*8
    ld.d    $t1, $t0, TRAP_SCRATCH_SAVED_USER_T2
    st.d    $t1, $sp, 14*8

    csrrd   $t1, CSR_ERA
    st.d    $t1, $sp, USER_CONTEXT_PC
    st.d    $t1, $sp, USER_CONTEXT_RESTART_PC
    csrrd   $t1, CSR_PRMD
    st.d    $t1, $sp, USER_CONTEXT_SSTATUS

    csrrd   $t2, CSR_ERA
    st.d    $t2, $sp, USER_CONTEXT_TRAP_RECORD_ERA
    csrrd   $t2, CSR_PRMD
    st.d    $t2, $sp, USER_CONTEXT_TRAP_RECORD_PRMD
    csrrd   $t2, CSR_ESTAT
    st.d    $t2, $sp, USER_CONTEXT_TRAP_RECORD_ESTAT
    csrrd   $t2, CSR_BADV
    st.d    $t2, $sp, USER_CONTEXT_TRAP_RECORD_BADV

    ld.d    $t2, $t0, TRAP_SCRATCH_KERNEL_STACK_TOP
    csrwr   $t0, CSR_KS0
    move    $a0, $sp
    move    $sp, $t2
    bl      handle_trap_rust

    b       restore_user_context_locked

    .align 6
    .globl tlbr_entry
tlbr_entry:
    csrwr   $t0, CSR_TLBRSAVE
    csrrd   $t0, 0x01b
    lddir   $t0, $t0, 3
    lddir   $t0, $t0, 1
    ldpte   $t0, 0
    ldpte   $t0, 1
    tlbfill
    csrrd   $t0, CSR_TLBRSAVE
    ertn

tlbr_fault_entry:
    csrrd   $t0, CSR_KS0
    beqz    $t0, kernel_trap_panic

    st.d    $sp, $t0, TRAP_SCRATCH_SAVED_USER_SP
    st.d    $t1, $t0, TRAP_SCRATCH_SAVED_USER_T1
    st.d    $t2, $t0, TRAP_SCRATCH_SAVED_USER_T2

    csrrd   $t1, CSR_TLBRPRMD
    andi    $t1, $t1, PRMD_PPLV_MASK
    li.w    $t2, PRMD_PPLV_USER
    beq     $t1, $t2, tlbr_user_fault
    b       kernel_trap_panic

tlbr_user_fault:
    ld.d    $sp, $t0, TRAP_SCRATCH_USER_CONTEXT
    bnez    $sp, tlbr_save_context
    b       kernel_trap_panic

tlbr_save_context:
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
    csrrd   $t1, CSR_TLBRSAVE
    st.d    $t1, $sp, 12*8
    ld.d    $t1, $t0, TRAP_SCRATCH_SAVED_USER_T1
    st.d    $t1, $sp, 13*8
    ld.d    $t1, $t0, TRAP_SCRATCH_SAVED_USER_T2
    st.d    $t1, $sp, 14*8

    csrrd   $t1, CSR_TLBRERA
    st.d    $t1, $sp, USER_CONTEXT_PC
    st.d    $t1, $sp, USER_CONTEXT_RESTART_PC
    csrrd   $t1, CSR_TLBRPRMD
    st.d    $t1, $sp, USER_CONTEXT_SSTATUS

    csrrd   $t2, CSR_TLBRERA
    st.d    $t2, $sp, USER_CONTEXT_TRAP_RECORD_ERA
    csrrd   $t2, CSR_TLBRPRMD
    st.d    $t2, $sp, USER_CONTEXT_TRAP_RECORD_PRMD
    li.w    $t2, TLBR_FALLBACK_ESTAT_PIS
    st.d    $t2, $sp, USER_CONTEXT_TRAP_RECORD_ESTAT
    csrrd   $t2, CSR_TLBRBADV
    st.d    $t2, $sp, USER_CONTEXT_TRAP_RECORD_BADV

    ld.d    $t2, $t0, TRAP_SCRATCH_KERNEL_STACK_TOP
    move    $a0, $sp
    move    $sp, $t2
    addi.d  $sp, $sp, -16
    st.d    $a0, $sp, 0
    bl      handle_tlbr_trap_record
    ld.d    $a0, $sp, 0
    addi.d  $sp, $sp, 16
    bl      handle_trap_rust

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

    csrrd   $t1, CSR_KS0
    beqz    $t1, kernel_trap_panic
    st.d    $t0, $t1, TRAP_SCRATCH_USER_CONTEXT

    ld.d    $t1, $t0, USER_CONTEXT_PC
    csrwr   $t1, CSR_ERA
    ld.d    $t1, $t0, USER_CONTEXT_SSTATUS
    csrwr   $t1, CSR_PRMD
    ld.d    $t1, $t0, USER_CONTEXT_PC
    csrwr   $t1, CSR_TLBRERA
    ld.d    $t1, $t0, USER_CONTEXT_SSTATUS
    csrwr   $t1, CSR_TLBRPRMD

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
