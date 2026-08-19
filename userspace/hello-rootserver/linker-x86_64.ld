OUTPUT_ARCH(i386:x86-64)
ENTRY(_start)

SECTIONS
{
  . = 0x100000;

  .text : ALIGN(0x1000) {
    KEEP(*(.text._start))
    *(.text .text.*)
  }

  .rodata : ALIGN(0x1000) {
    *(.rodata .rodata.*)
  }

  .data : ALIGN(0x1000) {
    *(.data .data.*)
  }

  .bss : ALIGN(0x1000) {
    __bss_start = .;
    *(.bss .bss.*)
    *(COMMON)
    __bss_end = .;
  }

  /DISCARD/ : {
    *(.eh_frame .eh_frame.*)
    *(.comment)
  }
}
