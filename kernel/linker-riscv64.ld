/*
 * Linker script for the Rust kernel after we are loaded by the seL4 C
 * elfloader. The elfloader sets up Sv39 paging and a 1GiB kernel window
 * before jumping to our `_start` at the kernel ELF window VA.
 *
 * Layout (matches the C kernel `common_riscv.lds`):
 *   PHYS_BASE_RAW         = 0x80200000          (LMA)
 *   PPTR_TOP              = 0xFFFFFFFF80000000
 *   KERNEL_ELF_BASE       = PPTR_TOP + (PHYS_BASE_RAW % 1GiB)
 *                         = 0xFFFFFFFF80200000  (VMA)
 *
 * Each section is linked at its kernel-window VA and given an `AT()`
 * physical address so the ELF program headers carry the correct LMA
 * the elfloader expects.
 */

OUTPUT_ARCH(riscv)
OUTPUT_FORMAT("elf64-littleriscv", "elf64-littleriscv", "elf64-littleriscv")
ENTRY(_start)

KERNEL_ELF_BASE = 0xFFFFFFFF80200000;
KERNEL_PADDR_BASE = 0x80200000;
KERNEL_OFFSET = KERNEL_ELF_BASE - KERNEL_PADDR_BASE;

PHDRS {
    boot   PT_LOAD FLAGS(0x7);   /* RWX during boot                  */
    text   PT_LOAD FLAGS(0x5);   /* R-X                              */
    rodata PT_LOAD FLAGS(0x4);   /* R--                              */
    data   PT_LOAD FLAGS(0x6);   /* RW-                              */
    bss    PT_LOAD FLAGS(0x6);   /* RW- (zero-init, not in file)     */
}

SECTIONS {
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
        *(.srodata .srodata.*)
    } :rodata

    . = ALIGN(4K);

    .data . : AT(ADDR(.data) - KERNEL_OFFSET) {
        *(.data .data.*)
        *(.sdata .sdata.*)
    } :data

    . = ALIGN(4K);

    .bss . (NOLOAD) : AT(ADDR(.bss) - KERNEL_OFFSET) {
        __bss_start = .;
        *(.bss .bss.*)
        *(.sbss .sbss.*)
        *(COMMON)

        . = ALIGN(16);
        __stack_bottom = .;
        . += 512K;        /* 8 per-hart stacks, 64 KiB each. */
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
        *(.riscv.attributes)
        *(.gnu.hash)
        *(.hash)
    }
}
