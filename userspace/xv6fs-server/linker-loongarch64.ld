OUTPUT_ARCH(loongarch)
ENTRY(_start)

SECTIONS
{
  . = 0x100000;

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
    __bss_start = .;
    *(.sbss .sbss.*)
    *(.bss .bss.*)
    *(COMMON)
    __bss_end = .;
  }

  /DISCARD/ : {
    *(.eh_frame .eh_frame.*)
    *(.comment)
  }
}
