OUTPUT_ARCH(riscv)
ENTRY(_start)

PHDRS {
  text PT_LOAD FLAGS(5);
  rodata PT_LOAD FLAGS(4);
  data PT_LOAD FLAGS(6);
}

SECTIONS {
  . = 0x00400000;

  .text : ALIGN(4K) {
    *(.text.entry)
    *(.text .text.*)
  } :text

  .rodata : ALIGN(4K) {
    *(.rodata .rodata.*)
    *(.srodata .srodata.*)
  } :rodata

  .data : ALIGN(4K) {
    *(.sdata .sdata.*)
    *(.data .data.*)
  } :data

  .bss (NOLOAD) : ALIGN(4K) {
    __bss_start = .;
    *(.sbss .sbss.*)
    *(.bss .bss.*)
    *(COMMON)
    __bss_end = .;
  } :data
}
