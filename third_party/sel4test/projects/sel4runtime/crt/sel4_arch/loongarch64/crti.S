/*
 * Copyright 2019, Data61, CSIRO (ABN 41 687 119 230)
 *
 * SPDX-License-Identifier: BSD-2-Clause
 */
.section .init
.global _init
_init:
    addi.d  $sp, $sp, -16
    st.d    $fp, $sp, 0
    st.d    $ra, $sp, 8
    move    $fp, $sp

.section .fini
.global _fini
_fini:
    addi.d  $sp, $sp, -16
    st.d    $fp, $sp, 0
    st.d    $ra, $sp, 8
    move    $fp, $sp
