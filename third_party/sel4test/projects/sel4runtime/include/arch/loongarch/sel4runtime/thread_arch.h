/*
 * Copyright 2019, Data61, CSIRO (ABN 41 687 119 230)
 *
 * SPDX-License-Identifier: BSD-2-Clause
 */
#include <sel4runtime/stdint.h>

static inline sel4runtime_uintptr_t sel4runtime_read_tp(void)
{
    register sel4runtime_uintptr_t reg __asm__("$tp");
    return reg;
}

static inline void sel4runtime_write_tp(sel4runtime_uintptr_t reg)
{
    __asm__ __volatile__("move $tp, %0" :: "r"(reg));
}

static inline sel4runtime_uintptr_t sel4runtime_get_tls_base(void)
{
    return sel4runtime_read_tp();
}

static inline void sel4runtime_set_tls_base(sel4runtime_uintptr_t tls_base)
{
    sel4runtime_write_tp(tls_base);
}

#define TLS_ABOVE_TP
