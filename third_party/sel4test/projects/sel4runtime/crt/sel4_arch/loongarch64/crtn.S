/*
 * Copyright 2019, Data61, CSIRO (ABN 41 687 119 230)
 *
 * SPDX-License-Identifier: BSD-2-Clause
 */
.section .init
    move    $sp, $fp
    ld.d    $ra, $sp, 8
    ld.d    $fp, $sp, 0
    addi.d  $sp, $sp, 16
    ret

.section .fini
    move    $sp, $fp
    ld.d    $ra, $sp, 8
    ld.d    $fp, $sp, 0
    addi.d  $sp, $sp, 16
    ret
