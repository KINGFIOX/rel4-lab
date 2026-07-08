/*
 * Copyright 2020, Data61, CSIRO (ABN 41 687 119 230)
 * Copyright 2015, 2016 Hesham Almatary <heshamelmatary@gmail.com>
 *
 * SPDX-License-Identifier: GPL-2.0-only
 */

#include <config.h>
#include <machine/assembler.h>
#include <arch/machine/hardware.h>
#include <arch/api/syscall.h>
#include <arch/machine/registerset.h>
#include <util.h>

#define REGBYTES (CONFIG_WORD_SIZE / 8)

.extern c_handle_syscall
.extern c_handle_fastpath_reply_recv
.extern c_handle_fastpath_call
.extern c_handle_interrupt
.extern c_handle_exception

.section .text.traps, "ax"
BEGIN_FUNC(trap_entry)
  b trap_entry
END_FUNC(trap_entry)
