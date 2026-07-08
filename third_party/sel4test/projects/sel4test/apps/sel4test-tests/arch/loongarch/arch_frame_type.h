/*
 * Copyright 2018, Data61, CSIRO (ABN 41 687 119 230)
 *
 * SPDX-License-Identifier: BSD-2-Clause
 */
#pragma once

#include <stdint.h>
#include <sel4/types.h>
#include <sel4/sel4.h>

/* This list must be ordered by size - highest first. */
static const frame_type_t frame_types[] = {
    { seL4_LoongArch_4K_Page, 0, seL4_PageBits, },
};
