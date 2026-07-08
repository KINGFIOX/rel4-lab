/*
 * Copyright 2018, Data61, CSIRO (ABN 41 687 119 230)
 *
 * SPDX-License-Identifier: BSD-2-Clause
 */
#pragma once

#include <autoconf.h>
#include <sel4/sel4_arch/constants.h>
#include <stddef.h>
#include <utils/attribute.h>

/* ordered list of page sizes for this architecture */
static const UNUSED size_t sel4_page_sizes[] = {
    seL4_PageBits,
    seL4_LargePageBits,
#if CONFIG_PT_LEVELS > 2
    seL4_HugePageBits,
#endif
#if CONFIG_PT_LEVELS > 3
    seL4_TeraPageBits,
#endif
};

#define seL4_ARCH_Page_Map               seL4_LoongArch_Page_Map
#define seL4_ARCH_Page_Unmap             seL4_LoongArch_Page_Unmap
#define seL4_ARCH_Page_GetAddress        seL4_LoongArch_Page_GetAddress
#define seL4_ARCH_Page_GetAddress_t      seL4_LoongArch_Page_GetAddress_t
#define seL4_ARCH_PageTable_Map          seL4_LoongArch_PageTable_Map
#define seL4_ARCH_PageTable_Unmap        seL4_LoongArch_PageTable_Unmap
#define seL4_ARCH_ASIDPool_Assign        seL4_LoongArch_ASIDPool_Assign
#define seL4_ARCH_ASIDControl_MakePool   seL4_LoongArch_ASIDControl_MakePool
#define seL4_ARCH_PageTableObject        seL4_LoongArch_PageTableObject
#define seL4_ARCH_PageDirectoryObject    seL4_LoongArch_PageTableObject
#define seL4_ARCH_Default_VMAttributes   seL4_LoongArch_Default_VMAttributes
#define seL4_ARCH_VMAttributes           seL4_LoongArch_VMAttributes
#define seL4_ARCH_4KPage                 seL4_LoongArch_4K_Page
#define seL4_ARCH_Uncached_VMAttributes  0
#define seL4_ARCH_LargePageObject        seL4_LoongArch_Mega_Page
#define ARCHPageGetAddress               LoongArchPageGetAddress
