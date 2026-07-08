/*
 * Copyright 2019, Data61, CSIRO (ABN 41 687 119 230)
 *
 * SPDX-License-Identifier: BSD-2-Clause
 */
.section .text
.global _start
_start:
    li.d    $fp, 0
    move    $a0, $sp
    la.local $t0, __sel4_start_c
    jirl    $ra, $t0, 0

1:
    b       1b
