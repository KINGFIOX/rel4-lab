#![allow(dead_code)]

use core::arch::asm;

pub fn sscratch() -> usize {
    0
}

pub fn set_sscratch(_value: usize) {}

pub fn dbar() {
    core::sync::atomic::compiler_fence(core::sync::atomic::Ordering::SeqCst);
}

pub fn sfence_vma_all() {
    unsafe {
        let cr3: usize;
        asm!("mov {}, cr3", out(reg) cr3, options(nostack, preserves_flags));
        asm!("mov cr3, {}", in(reg) cr3, options(nostack, preserves_flags));
    }
}

pub fn sfence_vma_va(vaddr: usize) {
    unsafe {
        asm!("invlpg [{}]", in(reg) vaddr, options(nostack, preserves_flags));
    }
}

pub fn sfence_vma_asid(_asid: usize) {
    sfence_vma_all();
}

pub fn fence_i() {
    core::sync::atomic::compiler_fence(core::sync::atomic::Ordering::SeqCst);
}
