// Trap entry/exit for x86_64. Layout matches sel4_arch UserContext.regs.

    .section .text.traps
    .align 16

    .equ TRAP_SCRATCH_KERNEL_STACK_TOP, 0
    .equ TRAP_SCRATCH_USER_CONTEXT, 8
    .equ TRAP_SCRATCH_SAVED_USER_SP, 16
    .equ TRAP_SCRATCH_SAVED_USER_T1, 24
    .equ TRAP_SCRATCH_SAVED_USER_T2, 32

    .equ UC_RDI, 0
    .equ UC_RSI, 8
    .equ UC_RAX, 16
    .equ UC_RBX, 24
    .equ UC_RBP, 32
    .equ UC_R12, 40
    .equ UC_R13, 48
    .equ UC_R14, 56
    .equ UC_RDX, 64
    .equ UC_R10, 72
    .equ UC_R8, 80
    .equ UC_R9, 88
    .equ UC_R15, 96
    .equ UC_FLAGS, 104
    .equ UC_NEXT_IP, 112
    .equ UC_ERROR, 120
    .equ UC_RSP, 128
    .equ UC_FAULT_IP, 136
    .equ UC_R11, 144
    .equ UC_RCX, 152
    .equ UC_CS, 160
    .equ UC_SS, 168
    .equ UC_FS_BASE, 176
    .equ UC_GS_BASE, 184
    .equ UC_PC, 192

    .equ USER_CS, 0x23
    .equ USER_SS, 0x1b
    .equ SYSCALL_CAUSE, 0x10000
    .equ SYSCALL_ERROR, -1

    .globl syscall_entry
syscall_entry:
    swapgs
    movq    %rsp, %gs:TRAP_SCRATCH_SAVED_USER_SP
    movq    %gs:TRAP_SCRATCH_KERNEL_STACK_TOP, %rsp
    movq    %rbx, %gs:TRAP_SCRATCH_SAVED_USER_T1
    movq    %gs:TRAP_SCRATCH_USER_CONTEXT, %rbx
    testq   %rbx, %rbx
    jz      kernel_trap_panic
    movq    %rdi, UC_RDI(%rbx)
    movq    %rsi, UC_RSI(%rbx)
    movq    %rax, UC_RAX(%rbx)
    movq    %rbp, UC_RBP(%rbx)
    movq    %r12, UC_R12(%rbx)
    movq    %r13, UC_R13(%rbx)
    movq    %r14, UC_R14(%rbx)
    movq    %rdx, UC_RDX(%rbx)
    movq    %r10, UC_R10(%rbx)
    movq    %r8, UC_R8(%rbx)
    movq    %r9, UC_R9(%rbx)
    movq    %r15, UC_R15(%rbx)
    movq    %r11, UC_FLAGS(%rbx)
    movq    %rcx, UC_NEXT_IP(%rbx)
    movq    %rcx, UC_FAULT_IP(%rbx)
    movq    %rcx, UC_PC(%rbx)
    movq    %r11, UC_R11(%rbx)
    movq    %rcx, UC_RCX(%rbx)
    movq    $SYSCALL_ERROR, UC_ERROR(%rbx)
    movq    $USER_CS, UC_CS(%rbx)
    movq    $USER_SS, UC_SS(%rbx)
    movq    %gs:TRAP_SCRATCH_SAVED_USER_SP, %rax
    movq    %rax, UC_RSP(%rbx)
    movq    %gs:TRAP_SCRATCH_SAVED_USER_T1, %rax
    movq    %rax, UC_RBX(%rbx)
    rdfsbase %rax
    movq    %rax, UC_FS_BASE(%rbx)
    movq    %rbx, %rdi
    movl    $SYSCALL_CAUSE, %esi
    andq    $~15, %rsp
    call    handle_trap_rust
    jmp     restore_user_context_locked

    .globl interrupt_common
interrupt_common:
    testb   $3, 24(%rsp)
    jz      kernel_interrupt
    swapgs
    movq    %rbx, %gs:TRAP_SCRATCH_SAVED_USER_T1
    movq    %gs:TRAP_SCRATCH_USER_CONTEXT, %rbx
    testq   %rbx, %rbx
    jz      kernel_trap_panic
    movq    %rdi, UC_RDI(%rbx)
    movq    %rsi, UC_RSI(%rbx)
    movq    %rax, UC_RAX(%rbx)
    movq    %rbp, UC_RBP(%rbx)
    movq    %r12, UC_R12(%rbx)
    movq    %r13, UC_R13(%rbx)
    movq    %r14, UC_R14(%rbx)
    movq    %rdx, UC_RDX(%rbx)
    movq    %r10, UC_R10(%rbx)
    movq    %r8, UC_R8(%rbx)
    movq    %r9, UC_R9(%rbx)
    movq    %r15, UC_R15(%rbx)
    movq    %rcx, UC_RCX(%rbx)
    movq    %r11, UC_R11(%rbx)
    movq    0(%rsp), %rax
    movq    %rax, %rsi
    movq    8(%rsp), %rax
    movq    %rax, UC_ERROR(%rbx)
    movq    16(%rsp), %rax
    movq    %rax, UC_NEXT_IP(%rbx)
    movq    %rax, UC_FAULT_IP(%rbx)
    movq    %rax, UC_PC(%rbx)
    movq    24(%rsp), %rax
    movq    %rax, UC_CS(%rbx)
    movq    32(%rsp), %rax
    movq    %rax, UC_FLAGS(%rbx)
    movq    40(%rsp), %rax
    movq    %rax, UC_RSP(%rbx)
    movq    48(%rsp), %rax
    movq    %rax, UC_SS(%rbx)
    movq    %gs:TRAP_SCRATCH_SAVED_USER_T1, %rax
    movq    %rax, UC_RBX(%rbx)
    rdfsbase %rax
    movq    %rax, UC_FS_BASE(%rbx)
    movq    %gs:TRAP_SCRATCH_KERNEL_STACK_TOP, %rsp
    movq    %rbx, %rdi
    andq    $~15, %rsp
    call    handle_trap_rust
    jmp     restore_user_context_locked

kernel_interrupt:
    pushq   %rax
    pushq   %rcx
    pushq   %rdx
    pushq   %rsi
    pushq   %rdi
    pushq   %r8
    pushq   %r9
    pushq   %r10
    pushq   %r11
    pushq   %rbp
    movq    %rsp, %rbp
    movq    80(%rbp), %rdi
    movq    96(%rbp), %rsi
    andq    $~15, %rsp
    subq    $8, %rsp
    call    handle_kernel_irq_rust
    movq    %rbp, %rsp
    popq    %rbp
    popq    %r11
    popq    %r10
    popq    %r9
    popq    %r8
    popq    %rdi
    popq    %rsi
    popq    %rdx
    popq    %rcx
    popq    %rax
    addq    $16, %rsp
    iretq

    .globl restore_user_context_locked
restore_user_context_locked:
    subq    $16, %rsp
    movq    %rdi, (%rsp)
    call    kernel_unlock_for_user_restore
    movq    (%rsp), %rdi
    addq    $16, %rsp
    jmp     restore_user_context

    .globl restore_user_context
restore_user_context:
    movq    %rdi, %r15
    movq    %rdi, %gs:TRAP_SCRATCH_USER_CONTEXT
    movq    UC_FS_BASE(%r15), %rax
    wrfsbase %rax
    movq    UC_ERROR(%r15), %rax
    cmpq    $SYSCALL_ERROR, %rax
    je      restore_sysret

restore_iret:
    cmpq    $0, UC_CS(%r15)
    jne     1f
    movq    $USER_CS, UC_CS(%r15)
    movq    $USER_SS, UC_SS(%r15)
1:
    movq    %gs:TRAP_SCRATCH_KERNEL_STACK_TOP, %rsp
    pushq   UC_SS(%r15)
    pushq   UC_RSP(%r15)
    pushq   UC_FLAGS(%r15)
    pushq   UC_CS(%r15)
    pushq   UC_PC(%r15)
    movq    UC_RSI(%r15), %rsi
    movq    UC_RAX(%r15), %rax
    movq    UC_RBX(%r15), %rbx
    movq    UC_RBP(%r15), %rbp
    movq    UC_R12(%r15), %r12
    movq    UC_R13(%r15), %r13
    movq    UC_R14(%r15), %r14
    movq    UC_RDX(%r15), %rdx
    movq    UC_R10(%r15), %r10
    movq    UC_R8(%r15), %r8
    movq    UC_R9(%r15), %r9
    movq    UC_RCX(%r15), %rcx
    movq    UC_R11(%r15), %r11
    movq    UC_RDI(%r15), %rdi
    movq    UC_R15(%r15), %r15
    swapgs
    iretq

restore_sysret:
    movq    UC_NEXT_IP(%r15), %rcx
    movq    UC_FLAGS(%r15), %r11
    movq    UC_RSI(%r15), %rsi
    movq    UC_RAX(%r15), %rax
    movq    UC_RBX(%r15), %rbx
    movq    UC_RBP(%r15), %rbp
    movq    UC_R12(%r15), %r12
    movq    UC_R13(%r15), %r13
    movq    UC_R14(%r15), %r14
    movq    UC_RDX(%r15), %rdx
    movq    UC_R10(%r15), %r10
    movq    UC_R8(%r15), %r8
    movq    UC_R9(%r15), %r9
    movq    UC_RSP(%r15), %rsp
    movq    UC_RDI(%r15), %rdi
    movq    UC_R15(%r15), %r15
    swapgs
    sysretq

    .macro ISR_NOERR n
    .align 16
    .globl isr_\n
isr_\n:
    pushq   $0
    pushq   $\n
    jmp     interrupt_common
    .endm

    .macro ISR_ERR n
    .align 16
    .globl isr_\n
isr_\n:
    pushq   $\n
    jmp     interrupt_common
    .endm

    ISR_NOERR 0
    ISR_NOERR 1
    ISR_NOERR 2
    ISR_NOERR 3
    ISR_NOERR 4
    ISR_NOERR 5
    ISR_NOERR 6
    ISR_NOERR 7
    ISR_ERR 8
    ISR_NOERR 9
    ISR_ERR 10
    ISR_ERR 11
    ISR_ERR 12
    ISR_ERR 13
    ISR_ERR 14
    ISR_NOERR 15
    ISR_NOERR 16
    ISR_ERR 17
    ISR_NOERR 18
    ISR_NOERR 19
    ISR_NOERR 20
    ISR_NOERR 21
    ISR_NOERR 22
    ISR_NOERR 23
    ISR_NOERR 24
    ISR_NOERR 25
    ISR_NOERR 26
    ISR_NOERR 27
    ISR_NOERR 28
    ISR_NOERR 29
    ISR_NOERR 30
    ISR_NOERR 31

    .altmacro
    .macro IRQ_STUB n
    .align 16
    .globl isr_\n
isr_\n:
    pushq   $0
    pushq   $\n
    jmp     interrupt_common
    .endm

    .set IRQVEC, 32
    .rept 224
    IRQ_STUB %IRQVEC
    .set IRQVEC, IRQVEC + 1
    .endr

    .section .rodata
    .align 8
    .globl isr_table
isr_table:
    .macro ISR_QUAD n
    .quad isr_\n
    .endm
    .set ISRIDX, 0
    .rept 256
    ISR_QUAD %ISRIDX
    .set ISRIDX, ISRIDX + 1
    .endr
