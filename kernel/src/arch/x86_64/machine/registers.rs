#![allow(dead_code)]

use core::arch::asm;

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

/// Staged kernel GS/scratch slot. The real trap path will program `IA32_KERNEL_GS_BASE`.
pub fn current_scratch() -> usize {
    0
}

pub fn set_current_scratch(_value: usize) {}

pub fn full_memory_barrier() {
    core::sync::atomic::compiler_fence(core::sync::atomic::Ordering::SeqCst);
}
