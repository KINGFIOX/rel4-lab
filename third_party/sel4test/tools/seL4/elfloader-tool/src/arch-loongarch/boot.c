/*
 * Copyright 2020, Data61, CSIRO (ABN 41 687 119 230)
 * Copyright 2021, HENSOLDT Cyber
 *
 * SPDX-License-Identifier: GPL-2.0-only
 */
#include <autoconf.h>
#include <elfloader/gen_config.h>

#include <abort.h>
#include <binaries/elf/elf.h>
#include <cpio/cpio.h>
#include <elfloader.h>
#include <types.h>

struct image_info kernel_info;
struct image_info user_info;

void const *dtb = NULL;
size_t dtb_size = 0;

void NORETURN abort(void)
{
    printf("HALT due to call to abort()\n");

    for (;;) {
        asm volatile("idle 0" ::: "memory");
    }

    UNREACHABLE();
}

static int run_elfloader(int hart_id, void *bootloader_dtb)
{
    unsigned int num_apps = 0;
    int ret = load_images(&kernel_info, &user_info, 1, &num_apps,
                          bootloader_dtb, &dtb, &dtb_size);
    if (ret != 0) {
        printf("ERROR: image loading failed, code %d\n", ret);
        return -1;
    }

    if (num_apps != 1) {
        printf("ERROR: expected to load just 1 app, actually loaded %u apps\n", num_apps);
        return -1;
    }

    printf("Jumping to kernel-image entry point...\n\n");
    ((init_loongarch_kernel_t)kernel_info.virt_entry)(user_info.phys_region_start,
                                                      user_info.phys_region_end,
                                                      user_info.phys_virt_offset,
                                                      user_info.virt_entry,
                                                      (word_t)dtb,
                                                      dtb_size,
                                                      hart_id,
                                                      0);

    printf("ERROR: Kernel returned back to the ELF Loader\n");
    return -1;
}

void main(int hart_id, void *bootloader_dtb)
{
    printf("ELF-loader started on LoongArch64 (hart %d, nodes %d)\n",
           hart_id, CONFIG_MAX_NUM_NODES);
    printf("  paddr=[%p..%p]\n", _text, (uintptr_t)_end - 1);

    int ret = run_elfloader(hart_id, bootloader_dtb);
    if (ret != 0) {
        printf("ERROR: ELF-loader failed, code %d\n", ret);
        abort();
        UNREACHABLE();
    }

    printf("ERROR: ELF-loader didn't hand over control\n");
    abort();
    UNREACHABLE();
}
