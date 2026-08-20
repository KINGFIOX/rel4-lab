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
    // SAFETY: reading CR2 has no side effect.
    unsafe {
        asm!("mov {}, cr2", out(reg) cr2, options(nostack, preserves_flags));
    }
    cr2
}

pub fn read_cr3() -> usize {
    let cr3: usize;
    // SAFETY: reading CR3 has no side effect.
    unsafe {
        asm!("mov {}, cr3", out(reg) cr3, options(nostack, preserves_flags));
    }
    cr3
}

pub fn write_cr3(value: usize) {
    // SAFETY: callers reach this through `switch_cr3`, which vouches for the
    // table; reloading CR3 also flushes non-global TLB entries.
    unsafe {
        asm!("mov cr3, {}", in(reg) value, options(nostack, preserves_flags));
    }
}

pub fn invlpg(vaddr: usize) {
    // SAFETY: invalidating a TLB entry only discards a cached translation.
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
    // SAFETY: reading an MSR the kernel is privileged for has no side effect.
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

/// Write a model-specific register.
///
/// # Safety
/// `msr` and `value` must be a combination the CPU accepts, and the caller
/// must accept whatever machine-wide effect it has.
pub unsafe fn wrmsr(msr: u32, value: u64) {
    let low = value as u32;
    let high = (value >> 32) as u32;
    // SAFETY: forwarded to the caller.
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
    // SAFETY: reading the GS base has no side effect.
    unsafe {
        asm!("rdgsbase {}", out(reg) value, options(nostack, preserves_flags));
    }
    value
}

/// Set the `GS` base, which is where the kernel keeps its per-core pointer.
///
/// # Safety
/// `value` must be the address of this core's trap scratch area, since trap
/// entry assembly dereferences it.
pub unsafe fn wrgsbase(value: usize) {
    // SAFETY: forwarded to the caller.
    unsafe {
        asm!("wrgsbase {}", in(reg) value, options(nostack, preserves_flags));
    }
}

pub fn current_scratch() -> usize {
    rdgsbase()
}

/// Publish this core's trap scratch address.
///
/// # Safety
/// `value` must be the address of this core's trap scratch area.
pub unsafe fn set_current_scratch(value: usize) {
    // SAFETY: forwarded to the caller. `IA32_KERNEL_GS_BASE` is cleared so
    // `swapgs` on trap entry finds a zero shadow rather than stale state.
    unsafe {
        wrgsbase(value);
        wrmsr(IA32_KERNEL_GS_BASE, 0);
    }
}

pub fn full_memory_barrier() {
    core::sync::atomic::compiler_fence(core::sync::atomic::Ordering::SeqCst);
}

/// Install a global descriptor table.
///
/// # Safety
/// `gdtr` must point at a valid `lgdt` operand describing a well-formed GDT
/// whose entries match the selectors the kernel goes on to load.
pub unsafe fn lgdt(gdtr: *const u8) {
    // SAFETY: forwarded to the caller.
    unsafe {
        asm!("lgdt [{}]", in(reg) gdtr, options(nostack, preserves_flags));
    }
}

/// Install an interrupt descriptor table.
///
/// # Safety
/// `idtr` must point at a valid `lidt` operand describing a fully populated
/// IDT whose gates name real entry points.
pub unsafe fn lidt(idtr: *const u8) {
    // SAFETY: forwarded to the caller.
    unsafe {
        asm!("lidt [{}]", in(reg) idtr, options(nostack, preserves_flags));
    }
}

/// Load the task register.
///
/// # Safety
/// `selector` must name a valid TSS descriptor in the current GDT.
pub unsafe fn ltr(selector: u16) {
    // SAFETY: forwarded to the caller.
    unsafe {
        asm!("ltr {0:x}", in(reg) selector, options(nostack, preserves_flags));
    }
}

/// Reload the data segment selectors.
///
/// # Safety
/// `selector` must name a valid data segment in the current GDT; loading a
/// bad `ss` faults on the next stack access.
pub unsafe fn load_ds_es_ss(selector: u16) {
    // SAFETY: forwarded to the caller.
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
    // SAFETY: reading the timestamp counter has no side effect.
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
