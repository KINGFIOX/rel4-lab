//! 16-bit AP startup at physical 0x8000, then long-mode kernel entry.

use core::sync::atomic::{AtomicU64, Ordering};

use crate::arch::x86_64::machine::lapic;
use crate::arch::x86_64::object::vspace;
use crate::kernel::smp::{SECONDARY_BOOT_READY, SECONDARY_BOOT_READY_MAGIC};

const TRAMPOLINE_PADDR: usize = 0x8000;
const MAILBOX_PADDR: usize = 0x8f00;
const SIPI_VECTOR: u8 = (TRAMPOLINE_PADDR >> 12) as u8;

const MAIL_GDTR32: usize = 0x00;
const MAIL_GDT32: usize = 0x10;
const MAIL_GDTR64: usize = 0x28;
const MAIL_GDT64: usize = 0x30;
const MAIL_CR3: usize = 0x50;
const MAIL_ENTRY: usize = 0x58;
const MAIL_STACK: usize = 0x60;
const MAIL_CORE: usize = 0x68;
const MAIL_CPU: usize = 0x70;
const MAIL_ALIVE: usize = 0x78;

static BOOT_CR3: AtomicU64 = AtomicU64::new(0);

pub fn save_boot_cr3() {
    BOOT_CR3.store(
        crate::arch::x86_64::machine::registers::read_cr3() as u64,
        Ordering::Release,
    );
}

fn mailbox_ptr() -> *mut u8 {
    vspace::paddr_to_pptr(MAILBOX_PADDR) as *mut u8
}

fn trampoline_ptr() -> *mut u8 {
    vspace::paddr_to_pptr(TRAMPOLINE_PADDR) as *mut u8
}

unsafe fn write_u16(base: *mut u8, off: usize, value: u16) {
    unsafe { core::ptr::write_unaligned(base.add(off) as *mut u16, value) };
}

unsafe fn write_u32(base: *mut u8, off: usize, value: u32) {
    unsafe { core::ptr::write_unaligned(base.add(off) as *mut u32, value) };
}

unsafe fn write_u64(base: *mut u8, off: usize, value: u64) {
    unsafe { core::ptr::write_unaligned(base.add(off) as *mut u64, value) };
}

unsafe fn read_u64(base: *const u8, off: usize) -> u64 {
    unsafe { core::ptr::read_unaligned(base.add(off) as *const u64) }
}

fn install_trampoline() {
    let code = trampoline_bytes();
    unsafe {
        let dst = trampoline_ptr();
        core::ptr::write_bytes(dst, 0, 0x1000);
        core::ptr::copy_nonoverlapping(code.as_ptr(), dst, code.len());
        let mail = mailbox_ptr();
        core::ptr::write_bytes(mail, 0, 0x100);
        write_u16(mail, MAIL_GDTR32, 24 - 1);
        write_u32(mail, MAIL_GDTR32 + 2, (MAILBOX_PADDR + MAIL_GDT32) as u32);
        write_u64(mail, MAIL_GDT32 + 8, 0x00cf_9a00_0000_ffff);
        write_u64(mail, MAIL_GDT32 + 16, 0x00cf_9200_0000_ffff);
        write_u16(mail, MAIL_GDTR64, 24 - 1);
        write_u32(mail, MAIL_GDTR64 + 2, (MAILBOX_PADDR + MAIL_GDT64) as u32);
        write_u64(mail, MAIL_GDT64 + 8, 0x00af_9a00_0000_ffff);
        write_u64(mail, MAIL_GDT64 + 16, 0x00af_9200_0000_ffff);
        write_u64(mail, MAIL_CR3, BOOT_CR3.load(Ordering::Acquire));
        write_u64(mail, MAIL_ENTRY, ap_long_entry as *const () as usize as u64);
    }
}

fn trampoline_bytes() -> [u8; 160] {
    // Real-mode stub assembled for origin 0x8000. See comments in-line.
    let mut code = [0u8; 160];
    let real: &[u8] = &[
        0xfa, // cli
        0x31, 0xc0, // xor ax, ax
        0x8e, 0xd8, // mov ds, ax
        0x8e, 0xc0, // mov es, ax
        0x8e, 0xd0, // mov ss, ax
        0x0f, 0x01, 0x16, 0x00, 0x8f, // lgdt [0x8f00]
        0x0f, 0x20, 0xc0, // mov eax, cr0
        0x66, 0x83, 0xc8, 0x01, // or eax, 1
        0x0f, 0x22, 0xc0, // mov cr0, eax
        0x66, 0xea, 0x20, 0x80, 0x00, 0x00, 0x08, 0x00, // ljmp 0x08:0x8020
    ];
    let prot: &[u8] = &[
        0x66, 0xb8, 0x10, 0x00, // mov ax, 0x10
        0x8e, 0xd8, // mov ds, ax
        0x8e, 0xc0, // mov es, ax
        0x8e, 0xd0, // mov ss, ax
        0xa1, 0x50, 0x8f, 0x00, 0x00, // mov eax, [0x8f50]
        0x0f, 0x22, 0xd8, // mov cr3, eax
        0x0f, 0x20, 0xe0, // mov eax, cr4
        0x83, 0xc8, 0x20, // or eax, 0x20
        0x0f, 0x22, 0xe0, // mov cr4, eax
        0xb9, 0x80, 0x00, 0x00, 0xc0, // mov ecx, IA32_EFER
        0x0f, 0x32, // rdmsr
        0x0d, 0x00, 0x09, 0x00, 0x00, // or eax, LME|NXE
        0x0f, 0x30, // wrmsr
        0x0f, 0x20, 0xc0, // mov eax, cr0
        0x0d, 0x00, 0x00, 0x00, 0x80, // or eax, PG
        0x0f, 0x22, 0xc0, // mov cr0, eax
        0x0f, 0x01, 0x15, 0x28, 0x8f, 0x00, 0x00, // lgdt [0x8f28]
        0xea, 0x80, 0x80, 0x00, 0x00, 0x08, 0x00, // ljmp 0x08:0x8080
    ];
    let long: &[u8] = &[
        0x48, 0xa1, 0x60, 0x8f, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, // mov rax, [0x8f60]
        0x48, 0x89, 0xc4, // mov rsp, rax
        0x48, 0xa1, 0x58, 0x8f, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, // mov rax, [0x8f58]
        0xff, 0xe0, // jmp rax
    ];
    code[..real.len()].copy_from_slice(real);
    code[0x20..0x20 + prot.len()].copy_from_slice(prot);
    code[0x80..0x80 + long.len()].copy_from_slice(long);
    code
}

fn delay() {
    let mut i = 0u32;
    while i < 2_000_000 {
        core::hint::spin_loop();
        i += 1;
    }
}

pub fn start_aps(num_nodes: usize) {
    install_trampoline();
    let mut core = 1usize;
    while core < num_nodes {
        unsafe {
            write_u64(mailbox_ptr(), MAIL_ALIVE, 0);
            write_u64(mailbox_ptr(), MAIL_CORE, core as u64);
            write_u64(mailbox_ptr(), MAIL_CPU, core as u64);
            write_u64(
                mailbox_ptr(),
                MAIL_STACK,
                crate::kernel::smp::kernel_stack_top_for_core(core) as u64,
            );
        }
        unsafe {
            core::arch::asm!("mfence", options(nostack, nomem, preserves_flags));
        }
        let dest = core as u32;
        lapic::send_init(dest);
        delay();
        lapic::send_sipi(dest, SIPI_VECTOR);
        delay();
        if unsafe { read_u64(mailbox_ptr(), MAIL_ALIVE) } == 0 {
            lapic::send_sipi(dest, SIPI_VECTOR);
            delay();
        }
        let mut spins = 0u32;
        while unsafe { read_u64(mailbox_ptr(), MAIL_ALIVE) } == 0 && spins < 10_000_000 {
            core::hint::spin_loop();
            spins += 1;
        }
        if unsafe { read_u64(mailbox_ptr(), MAIL_ALIVE) } == 0 {
            panic!("x86_64 AP {core} did not start");
        }
        core += 1;
    }
}

#[unsafe(no_mangle)]
pub extern "C" fn ap_long_entry() -> ! {
    let boot_mail = MAILBOX_PADDR as *mut u8;
    let stack = unsafe { read_u64(boot_mail, MAIL_STACK) } as usize;
    unsafe {
        core::arch::asm!(
            "mov {tmp}, cr0",
            "and {tmp}, {em_ts}",
            "or {tmp}, {mp}",
            "mov cr0, {tmp}",
            "mov {tmp}, cr4",
            "or {tmp}, {cr4_bits}",
            "mov cr4, {tmp}",
            "mov rsp, {stack}",
            em_ts = in(reg) !0xcu64,
            mp = const 0x2u64,
            cr4_bits = const 0x1_0600u64,
            stack = in(reg) stack,
            tmp = out(reg) _,
            options(nostack),
        );
        write_u64(boot_mail, MAIL_ALIVE, 1);
    }
    if let Some(root) = crate::kernel::smp::kernel_vspace_root() {
        unsafe { vspace::switch_cr3(root) };
    }
    // Match RISC-V secondaries: stay invisible to remote TLB/IPI until the
    // BSP publishes boot-complete. `init_current_cpu` sets `online`.
    while SECONDARY_BOOT_READY.load(Ordering::Acquire) != SECONDARY_BOOT_READY_MAGIC {
        core::hint::spin_loop();
    }
    // Re-read identity-mapped mailbox state through the kernel window after
    // the CR3 switch. Locals live on a stack slice the BSP can overlap.
    let mail = mailbox_ptr();
    let core_id = unsafe { read_u64(mail, MAIL_CORE) } as usize;
    let cpu_id = unsafe { read_u64(mail, MAIL_CPU) } as usize;
    crate::kernel::smp::init_current_cpu(cpu_id, core_id);
    crate::arch::x86_64::machine::fpu::init_current_core();
    crate::arch::x86_64::kernel::trap::install_trap_vector();
    crate::arch::x86_64::machine::irq::init_current_core();
    crate::arch::x86_64::kernel::trap::idle_scheduler_loop()
}
