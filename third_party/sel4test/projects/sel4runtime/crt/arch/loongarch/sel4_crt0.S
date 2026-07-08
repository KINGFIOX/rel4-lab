/*
 * Copyright 2019, Data61, CSIRO (ABN 41 687 119 230)
 *
 * SPDX-License-Identifier: BSD-2-Clause
 */
#include <sel4runtime/gen_config.h>

.section .text
.global _sel4_start
_sel4_start:
    la.local $sp, __stack_top
    la.local $t0, __sel4_start_root
    jirl    $ra, $t0, 0

1:
    b       1b

.section .bss
__stack_base:
    .align 16
    .space CONFIG_SEL4RUNTIME_ROOT_STACK
__stack_top:
