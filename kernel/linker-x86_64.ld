/*
 * Linker script for the x86_64 kernel. Low 1 MiB holds the Multiboot header
 * and the 32-bit trampoline; the high window at KERNEL_ELF_BASE is the
 * running kernel. GOT/PLT stay in the high image so RIP-relative helper
 * calls remain mapped after switch_vspace.
 */

OUTPUT_ARCH(i386:x86-64)
OUTPUT_FORMAT("elf64-x86-64", "elf64-x86-64", "elf64-x86-64")
ENTRY(_start)

KERNEL_ELF_BASE = 0xffffffff80200000;
KERNEL_PADDR_BASE = 0x00200000;
KERNEL_OFFSET = KERNEL_ELF_BASE - KERNEL_PADDR_BASE;

PHDRS {
    phys   PT_LOAD FLAGS(0x7);
    boot   PT_LOAD FLAGS(0x7);
    text   PT_LOAD FLAGS(0x5);
    rodata PT_LOAD FLAGS(0x4);
    data   PT_LOAD FLAGS(0x6);
    bss    PT_LOAD FLAGS(0x6);
}

SECTIONS {
    . = 1M;
    __phys_start = .;

    .mbh . : {
        KEEP(*(.mbh))
    } :phys

    . = ALIGN(16);
    .phys.text . : {
        KEEP(*(.phys.text))
        KEEP(*(.phys.text.*))
    } :phys

    .phys.rodata . : {
        KEEP(*(.phys.rodata))
        KEEP(*(.phys.rodata.*))
    } :phys

    . = ALIGN(4K);
    .phys.bss . (NOLOAD) : {
        __phys_bss_start = .;
        *(.phys.bss)
        *(.phys.bss.*)
        . = ALIGN(4K);
        __phys_bss_end = .;
    } :phys

    . = KERNEL_ELF_BASE;
    __kernel_start = .;

    .boot.text . : AT(ADDR(.boot.text) - KERNEL_OFFSET) {
        KEEP(*(.boot.text))
        KEEP(*(.boot.text.*))
    } :boot

    .boot.rodata . : AT(ADDR(.boot.rodata) - KERNEL_OFFSET) {
        *(.boot.rodata)
        *(.boot.rodata.*)
    } :boot

    . = ALIGN(8);
    .boot.data . : AT(ADDR(.boot.data) - KERNEL_OFFSET) {
        *(.boot.data)
        *(.boot.data.*)
    } :boot

    .boot.bss . : AT(ADDR(.boot.bss) - KERNEL_OFFSET) {
        __boot_bss_start = .;
        *(.boot.bss)
        *(.boot.bss.*)
        . = ALIGN(8);
        __boot_bss_end = .;
    } :boot

    . = ALIGN(4K);
    __boot_end = .;

    .text . : AT(ADDR(.text) - KERNEL_OFFSET) {
        *(.text.traps)
        *(.text .text.*)
    } :text

    . = ALIGN(4K);

    .rodata . : AT(ADDR(.rodata) - KERNEL_OFFSET) {
        *(.rodata .rodata.*)
    } :rodata

    . = ALIGN(4K);

    .data . : AT(ADDR(.data) - KERNEL_OFFSET) {
        *(.data .data.*)
        *(.data.rel.ro .data.rel.ro.*)
        *(.got .got.*)
        *(.got.plt)
    } :data

    .plt . : AT(ADDR(.plt) - KERNEL_OFFSET) {
        *(.plt .plt.*)
    } :text

    . = ALIGN(4K);

    .bss . (NOLOAD) : AT(ADDR(.bss) - KERNEL_OFFSET) {
        __bss_start = .;
        *(.bss .bss.*)
        *(COMMON)

        . = ALIGN(16);
        __stack_bottom = .;
        . += 512K;
        __stack_top = .;

        . = ALIGN(8);
        __bss_end = .;
    } :bss

    . = ALIGN(4K);
    __kernel_end = .;

    /DISCARD/ : {
        *(.eh_frame)
        *(.eh_frame_hdr)
        *(.note .note.*)
        *(.comment)
        *(.gnu.hash)
        *(.hash)
    }
}
