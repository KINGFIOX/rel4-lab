OUTPUT_ARCH(riscv)
ENTRY(_start)

SECTIONS
{
  . = 0x10000;

  .text : ALIGN(0x1000) {
    KEEP(*(.text._start))
    *(.text .text.*)
  }

  .rodata : ALIGN(0x1000) {
    *(.srodata .srodata.*)
    *(.rodata .rodata.*)
  }

  .data : ALIGN(0x1000) {
    *(.sdata .sdata.*)
    *(.data .data.*)
  }

  .bss : ALIGN(0x1000) {
    *(.sbss .sbss.*)
    *(.bss .bss.*)
    *(COMMON)
  }

  /DISCARD/ : {
    *(.eh_frame .eh_frame.*)
    *(.comment)
    *(.note .note.*)
  }
}
