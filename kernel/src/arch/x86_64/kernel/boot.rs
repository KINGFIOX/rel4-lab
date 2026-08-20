use core::arch::asm;

unsafe extern "C" {
    static __bss_start: u8;
    static __bss_end: u8;
    static __stack_top: u8;
    static x86_mb_magic: u32;
    static x86_mb_info: u32;
}

#[used]
#[unsafe(link_section = ".mbh")]
static MULTIBOOT_HEADER: [u32; 3] = [0x1bad_b002, 0x0000_0003, 0xe452_4ffb];

#[unsafe(naked)]
#[unsafe(no_mangle)]
#[unsafe(link_section = ".phys.text")]
pub unsafe extern "C" fn _start() -> ! {
    core::arch::naked_asm!(
        ".code32",
        "cli",
        "movl %eax, x86_mb_magic",
        "movl %ebx, x86_mb_info",
        "mov $0x3f8, %dx",
        "mov $0x41, %al",
        "out %al, %dx",
        "mov %eax, %edi",
        "mov %ebx, %esi",
        "lea x86_boot_stack_top, %esp",
        "push $0",
        "popf",
        "push $0",
        "push %esi",
        "push $0",
        "push %edi",
        "call 2f",
        "ljmp $8, $3f",
        "2:",
        "mov %cr0, %eax",
        "and $0x7fffffff, %eax",
        "mov %eax, %cr0",
        "xor %edx, %edx",
        "mov $x86_boot_pml4, %edi",
        "mov $1024, %ecx",
        "1:",
        "mov %edx, (%edi)",
        "add $4, %edi",
        "loop 1b",
        "mov $x86_boot_pdpt, %edi",
        "mov $1024, %ecx",
        "1:",
        "mov %edx, (%edi)",
        "add $4, %edi",
        "loop 1b",
        "mov $x86_boot_pml4, %edi",
        "mov $x86_boot_pdpt, %ecx",
        "or $0x7, %ecx",
        "mov %ecx, (%edi)",
        "mov %ecx, 0x800(%edi)",
        "mov %ecx, 4088(%edi)",
        "mov $x86_boot_pd, %ecx",
        "or $0x7, %ecx",
        "mov $x86_boot_pdpt, %edi",
        "mov %ecx, (%edi)",
        "mov %ecx, 4080(%edi)",
        "add $0x1000, %ecx",
        "mov %ecx, 8(%edi)",
        "add $0x1000, %ecx",
        "mov %ecx, 16(%edi)",
        "add $0x1000, %ecx",
        "mov %ecx, 24(%edi)",
        "mov $x86_boot_pd, %edi",
        "mov $2048, %ecx",
        "mov $0x87, %edx",
        "1:",
        "mov %edx, (%edi)",
        "add $0x200000, %edx",
        "add $8, %edi",
        "loop 1b",
        "mov $x86_boot_pml4, %eax",
        "mov %eax, %cr3",
        "mov %cr4, %eax",
        "or $0x20, %eax",
        "mov %eax, %cr4",
        "mov $0xc0000080, %ecx",
        "rdmsr",
        "or $0x900, %eax",
        "wrmsr",
        "mov %cr0, %eax",
        "or $0x80000000, %eax",
        "mov %eax, %cr0",
        "lgdt x86_gdt64_ptr",
        "ret",
        ".code64",
        "3:",
        "mov %cr0, %rax",
        "and $~0xc, %rax",
        "or $0x2, %rax",
        "mov %rax, %cr0",
        "mov %cr4, %rax",
        "or $0x10600, %rax",
        "mov %rax, %cr4",
        "mov $0x3f8, %dx",
        "mov $0x42, %al",
        "out %al, %dx",
        "movq (%rsp), %rdi",
        "movq 8(%rsp), %rsi",
        "movabs $x86_64_high_entry, %rax",
        "jmp *%rax",
        ".pushsection .phys.rodata, \"a\"",
        ".align 16",
        "x86_gdt64:",
        ".quad 0",
        ".word 0; .word 0; .byte 0; .byte 0x98; .byte 0x20; .byte 0",
        ".word 0; .word 0; .byte 0; .byte 0x90; .byte 0; .byte 0",
        "x86_gdt64_ptr:",
        ".word (3 * 8) - 1",
        ".long x86_gdt64",
        ".popsection",
        ".pushsection .phys.bss, \"aw\", @nobits",
        ".align 4096",
        "x86_boot_pml4: .skip 4096",
        "x86_boot_pdpt: .skip 4096",
        "x86_boot_pd: .skip 16384",
        ".align 16",
        "x86_boot_stack: .skip 4096",
        "x86_boot_stack_top:",
        ".globl x86_mb_magic",
        "x86_mb_magic: .long 0",
        ".globl x86_mb_info",
        "x86_mb_info: .long 0",
        ".popsection",
        options(att_syntax),
    );
}

#[unsafe(no_mangle)]
#[unsafe(link_section = ".boot.text")]
pub extern "C" fn x86_64_high_entry(multiboot_magic: usize, multiboot_info: usize) -> ! {
    serial_byte(b'C');
    // SAFETY: the boot path runs single-threaded on the boot core, before any
    // Rust object exists in the memory it touches.
    unsafe {
        core::arch::asm!("mov rsp, {}", in(reg) &__stack_top as *const u8 as usize, options(nostack));
    }
    clear_bss();
    crate::arch::x86_64::smp::trampoline::save_boot_cr3();
    // SAFETY: as above: reading the bootloader-provided structures and the
    // loaded image's headers as bytes.
    let (multiboot_magic, multiboot_info) = unsafe {
        let _ = (multiboot_magic, multiboot_info);
        (
            core::ptr::read_volatile(core::ptr::addr_of!(x86_mb_magic) as *const u32) as usize,
            core::ptr::read_volatile(core::ptr::addr_of!(x86_mb_info) as *const u32) as usize,
        )
    };
    let args = parse_multiboot_boot_args(multiboot_magic, multiboot_info);
    init_kernel(
        args.user_pstart,
        args.user_pend,
        args.pv_offset,
        args.user_ventry,
        args.dtb_pa,
        args.dtb_size,
        args.cpu_id,
        args.core_id,
    )
}

fn serial_byte(byte: u8) {
    // SAFETY: writing COM1's data port, used for early boot output.
    unsafe {
        core::arch::asm!(
            "out dx, al",
            in("dx") 0x3f8u16,
            in("al") byte,
            options(nomem, nostack, preserves_flags),
        );
    }
}

fn clear_bss() {
    // SAFETY: the linker-provided `.bss` range holds no initialised object yet.
    let mut p = unsafe { &__bss_start as *const u8 as *mut u64 };
    let end = unsafe { &__bss_end as *const u8 as usize };
    while (p as usize) < end {
        unsafe { p.write_volatile(0) };
        // SAFETY: as above: reading the bootloader-provided structures and the
        // loaded image's headers as bytes.
        p = unsafe { p.add(1) };
    }
}

#[repr(C, packed)]
struct MultibootModule {
    start: u32,
    end: u32,
    _name: u32,
    _reserved: u32,
}

#[repr(C, packed)]
struct MultibootInfoPart1 {
    flags: u32,
    _mem_lower: u32,
    _mem_upper: u32,
    _boot_device: u32,
    _cmdline: u32,
    mod_count: u32,
    mod_list: u32,
}

const MULTIBOOT_MAGIC: usize = 0x2bad_b002;
const MULTIBOOT_INFO_MODS_FLAG: u32 = 1 << 3;

fn parse_multiboot_boot_args(
    multiboot_magic: usize,
    multiboot_info: usize,
) -> crate::kernel::boot::BootArgs {
    if multiboot_magic != MULTIBOOT_MAGIC {
        panic!(
            "x86_64 bootloader did not provide Multiboot1 handoff (magic={multiboot_magic:#x} info={multiboot_info:#x})"
        );
    }
    // SAFETY: the bootloader placed this structure at the address it passed in
    // register state, and it stays mapped for the boot path.
    let mbi = unsafe { &*(multiboot_info as *const MultibootInfoPart1) };
    let flags = unsafe { core::ptr::addr_of!(mbi.flags).read_unaligned() };
    let mod_count = unsafe { core::ptr::addr_of!(mbi.mod_count).read_unaligned() };
    // SAFETY: as above: reading the bootloader-provided structures and the
    // loaded image's headers as bytes.
    let mod_list = unsafe { core::ptr::addr_of!(mbi.mod_list).read_unaligned() };
    if flags & MULTIBOOT_INFO_MODS_FLAG == 0 || mod_count == 0 {
        panic!("x86_64 bootloader did not provide a rootserver module");
    }
    let module = unsafe { &*(mod_list as usize as *const MultibootModule) };
    let user_pstart = unsafe { core::ptr::addr_of!(module.start).read_unaligned() as usize };
    let user_pend = unsafe { core::ptr::addr_of!(module.end).read_unaligned() as usize };
    if user_pend <= user_pstart {
        panic!("x86_64 rootserver module has invalid bounds");
    }
    let user_ventry = elf64_entry(user_pstart);
    let pv_offset = elf64_pv_offset(user_pstart);
    let user_pend = realize_elf_bss(user_pstart, user_pend, pv_offset);
    crate::kernel::boot::BootArgs {
        user_pstart,
        user_pend,
        pv_offset,
        user_ventry,
        dtb_pa: 0,
        dtb_size: 0,
        cpu_id: 0,
        core_id: 0,
    }
}

fn elf64_entry(image_paddr: usize) -> usize {
    let hdr = elf64_header(image_paddr);
    // SAFETY: the rootserver image was loaded by the bootloader at this address
    // and is read here as bytes.
    unsafe { core::ptr::read_unaligned(hdr.add(24) as *const u64) as usize }
}

/// Multiboot loads the ELF file bytes only. seL4-style `pv_offset` mapping
/// therefore misses `p_memsz - p_filesz` BSS. Zero that tail and extend the
/// reported physical image so `bringup_rootserver` maps it. Extra pages stay
/// below the 16 MiB FREE_RAM floor used for untypeds.
fn realize_elf_bss(image_paddr: usize, module_end: usize, pv_offset: usize) -> usize {
    let hdr = elf64_header(image_paddr);
    // SAFETY: the image's `.bss` range lies inside the module the bootloader
    // loaded, and no object lives there yet.
    let phoff = unsafe { core::ptr::read_unaligned(hdr.add(32) as *const u64) as usize };
    let phentsize = unsafe { core::ptr::read_unaligned(hdr.add(54) as *const u16) as usize };
    let phnum = unsafe { core::ptr::read_unaligned(hdr.add(56) as *const u16) as usize };
    let page_size = crate::arch::x86_64::machine::paging::PAGE_SIZE;
    let mut image_end = module_end;
    let mut i = 0usize;
    while i < phnum {
        // SAFETY: as above: reading the bootloader-provided structures and the
        // loaded image's headers as bytes.
        let phdr = unsafe { hdr.add(phoff + i * phentsize) };
        let p_type = unsafe { core::ptr::read_unaligned(phdr as *const u32) };
        if p_type == 1 {
            let p_offset = unsafe { core::ptr::read_unaligned(phdr.add(8) as *const u64) as usize };
            let p_filesz =
                unsafe { core::ptr::read_unaligned(phdr.add(32) as *const u64) as usize };
            let p_memsz = unsafe { core::ptr::read_unaligned(phdr.add(40) as *const u64) as usize };
            let p_vaddr = unsafe { core::ptr::read_unaligned(phdr.add(16) as *const u64) as usize };
            let file_end = image_paddr.wrapping_add(p_offset).wrapping_add(p_filesz);
            let bss_end = image_paddr.wrapping_add(p_offset).wrapping_add(p_memsz);
            let map_end = (p_vaddr.wrapping_add(p_memsz).wrapping_add(pv_offset) + page_size - 1)
                & !(page_size - 1);
            // Zero only this segment's BSS. Padding out to the next page would
            // wipe a later PT_LOAD that shares the page (hello's .got after
            // .rodata).
            if p_memsz > p_filesz {
                // SAFETY: as above: reading the bootloader-provided structures and the
                // loaded image's headers as bytes.
                unsafe {
                    core::ptr::write_bytes(file_end as *mut u8, 0, bss_end - file_end);
                }
            }
            if map_end > image_end {
                image_end = map_end;
            }
        }
        i += 1;
    }
    image_end
}

fn elf64_pv_offset(image_paddr: usize) -> usize {
    let hdr = elf64_header(image_paddr);
    // SAFETY: as `elf64_entry`: reading the loaded image's headers as bytes.
    let phoff = unsafe { core::ptr::read_unaligned(hdr.add(32) as *const u64) as usize };
    let phentsize = unsafe { core::ptr::read_unaligned(hdr.add(54) as *const u16) as usize };
    let phnum = unsafe { core::ptr::read_unaligned(hdr.add(56) as *const u16) as usize };
    let mut i = 0usize;
    while i < phnum {
        // SAFETY: as above: reading the bootloader-provided structures and the
        // loaded image's headers as bytes.
        let phdr = unsafe { hdr.add(phoff + i * phentsize) };
        let p_type = unsafe { core::ptr::read_unaligned(phdr as *const u32) };
        if p_type == 1 {
            let p_offset = unsafe { core::ptr::read_unaligned(phdr.add(8) as *const u64) as usize };
            let p_vaddr = unsafe { core::ptr::read_unaligned(phdr.add(16) as *const u64) as usize };
            return image_paddr.wrapping_add(p_offset).wrapping_sub(p_vaddr);
        }
        i += 1;
    }
    0
}

fn elf64_header(image_paddr: usize) -> *const u8 {
    let base = image_paddr as *const u8;
    // SAFETY: as `elf64_entry`: reading the loaded image's headers as bytes.
    let magic = unsafe { core::slice::from_raw_parts(base, 4) };
    if magic != b"\x7fELF" {
        panic!("x86_64 rootserver module is not an ELF image");
    }
    // SAFETY: as above: reading the bootloader-provided structures and the
    // loaded image's headers as bytes.
    let class = unsafe { base.add(4).read() };
    if class != 2 {
        panic!("x86_64 rootserver module is not ELF64");
    }
    base
}

#[unsafe(no_mangle)]
pub extern "C" fn init_kernel(
    user_pstart: usize,
    user_pend: usize,
    pv_offset: usize,
    user_ventry: usize,
    dtb_pa: usize,
    dtb_size: usize,
    cpu_id: usize,
    core_id: usize,
) -> ! {
    // SAFETY: the boot path runs single-threaded on the boot core.
    let _ = unsafe {
        (
            &__bss_start as *const u8,
            &__bss_end as *const u8,
            &__stack_top as *const u8,
        )
    };
    let args = crate::kernel::boot::BootArgs {
        user_pstart,
        user_pend,
        pv_offset,
        user_ventry,
        dtb_pa,
        dtb_size,
        cpu_id,
        core_id,
    };
    crate::kernel::boot::bringup_rootserver(&args)
}

pub fn halt() -> ! {
    loop {
        // SAFETY: halting this core.
        unsafe {
            asm!("hlt", options(nomem, nostack));
        }
    }
}
