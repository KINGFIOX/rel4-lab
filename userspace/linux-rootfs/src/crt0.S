    .section .text._start, "ax"
    .globl _start
    .type _start, @function
_start:
    ld a0, 0(sp)
    addi a1, sp, 8
    call main
    li a7, 93
    ecall
    1:
    j 1b
    .size _start, .-_start
