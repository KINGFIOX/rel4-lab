    .section .text._start, "ax"
    .globl _start
    .type _start, @function
_start:
    movq (%rsp), %rdi
    leaq 8(%rsp), %rsi
    call main
    movl $60, %eax
    xorl %edi, %edi
    syscall
1:
    jmp 1b
    .size _start, .-_start
