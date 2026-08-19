OUTPUT_ARCH(i386:x86-64)
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
  } :rodata

  .data : ALIGN(4K) {
    *(.data .data.*)
  } :data

  .bss (NOLOAD) : ALIGN(4K) {
    __bss_start = .;
    *(.bss .bss.*)
    *(COMMON)
    __bss_end = .;
  } :data
}
