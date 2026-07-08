/*
 * Copyright 2020, Data61, CSIRO (ABN 41 687 119 230)
 *
 * SPDX-License-Identifier: GPL-2.0-only
 */

#include <autoconf.h>
#include <elfloader/gen_config.h>

.extern main

.section ".text.start"

.global _start
_start:
  la.local $sp, core_stack_alloc_end
  addi.d   $sp, $sp, -16
  bl       clear_bss
  b        main

.global secondary_harts
secondary_harts:
1:
  idle     0
  b        1b
