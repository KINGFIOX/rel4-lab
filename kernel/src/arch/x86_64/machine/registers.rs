#![allow(dead_code)]

use core::arch::asm;

pub const IA32_EFER: u32 = 0xc000_0080;
pub const IA32_STAR: u32 = 0xc000_0081;
pub const IA32_LSTAR: u32 = 0xc000_0082;
pub const IA32_FMASK: u32 = 0xc000_0084;
pub const IA32_FS_BASE: u32 = 0xc000_0100;
pub const IA32_GS_BASE: u32 = 0xc000_0101;
pub const IA32_KERNEL_GS_BASE: u32 = 0xc000_0102;
pub const IA32_APIC_BASE: u32 = 0x1b;

pub const EFER_SCE: u64 = 1 << 0;
pub const EFER_LME: u64 = 1 << 8;
pub const EFER_NXE: u64 = 1 << 11;

pub fn read_cr2() -> usize {
    let cr2: usize;
    unsafe {
        asm!("mov {}, cr2", out(reg) cr2, options(nostack, preserves_flags));
    }
    cr2
}

pub fn read_cr3() -> usize {
    let cr3: usize;
    unsafe {
        asm!("mov {}, cr3", out(reg) cr3, options(nostack, preserves_flags));
    }
    cr3
}

pub fn write_cr3(value: usize) {
    unsafe {
        asm!("mov cr3, {}", in(reg) value, options(nostack, preserves_flags));
    }
}

pub fn invlpg(vaddr: usize) {
    unsafe {
        asm!("invlpg [{}]", in(reg) vaddr, options(nostack, preserves_flags));
    }
}

pub fn flush_tlb() {
    write_cr3(read_cr3());
}

pub fn rdmsr(msr: u32) -> u64 {
    let low: u32;
    let high: u32;
    unsafe {
        asm!(
            "rdmsr",
            in("ecx") msr,
            out("eax") low,
            out("edx") high,
            options(nostack, preserves_flags),
        );
    }
    ((high as u64) << 32) | (low as u64)
}

pub fn wrmsr(msr: u32, value: u64) {
    let low = value as u32;
    let high = (value >> 32) as u32;
    unsafe {
        asm!(
            "wrmsr",
            in("ecx") msr,
            in("eax") low,
            in("edx") high,
            options(nostack, preserves_flags),
        );
    }
}

pub fn rdgsbase() -> usize {
    let value: usize;
    unsafe {
        asm!("rdgsbase {}", out(reg) value, options(nostack, preserves_flags));
    }
    value
}

pub fn wrgsbase(value: usize) {
    unsafe {
        asm!("wrgsbase {}", in(reg) value, options(nostack, preserves_flags));
    }
}

pub fn current_scratch() -> usize {
    rdgsbase()
}

pub fn set_current_scratch(value: usize) {
    wrgsbase(value);
    wrmsr(IA32_KERNEL_GS_BASE, 0);
}

pub fn full_memory_barrier() {
    core::sync::atomic::compiler_fence(core::sync::atomic::Ordering::SeqCst);
}

pub fn lgdt(gdtr: *const u8) {
    unsafe {
        asm!("lgdt [{}]", in(reg) gdtr, options(nostack, preserves_flags));
    }
}

pub fn lidt(idtr: *const u8) {
    unsafe {
        asm!("lidt [{}]", in(reg) idtr, options(nostack, preserves_flags));
    }
}

pub fn ltr(selector: u16) {
    unsafe {
        asm!("ltr {0:x}", in(reg) selector, options(nostack, preserves_flags));
    }
}

pub fn load_ds_es_ss(selector: u16) {
    unsafe {
        asm!(
            "mov {0:x}, %ds",
            "mov {0:x}, %es",
            "mov {0:x}, %ss",
            in(reg) selector,
            options(att_syntax, nostack, preserves_flags),
        );
    }
}

pub fn rdtsc() -> u64 {
    let low: u32;
    let high: u32;
    unsafe {
        asm!(
            "rdtsc",
            out("eax") low,
            out("edx") high,
            options(nomem, nostack, preserves_flags),
        );
    }
    ((high as u64) << 32) | (low as u64)
}
