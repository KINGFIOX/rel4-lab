//! Thin helpers for RISC-V S-mode CSR access.

use core::arch::asm;

macro_rules! ro_csr {
    ($name:ident, $csr:literal) => {
        #[inline]
        #[allow(dead_code)]
        pub fn $name() -> usize {
            let v: usize;
            unsafe { asm!(concat!("csrr {0}, ", $csr), out(reg) v, options(nostack, nomem)) };
            v
        }
    };
}

macro_rules! rw_csr {
    ($read:ident, $write:ident, $csr:literal) => {
        ro_csr!($read, $csr);

        #[inline]
        #[allow(dead_code)]
        pub fn $write(v: usize) {
            unsafe { asm!(concat!("csrw ", $csr, ", {0}"), in(reg) v, options(nostack, nomem)) };
        }
    };
}

rw_csr!(sstatus, set_sstatus, "sstatus");
rw_csr!(stvec, set_stvec, "stvec");
rw_csr!(sscratch, set_sscratch, "sscratch");
rw_csr!(sepc, set_sepc, "sepc");
rw_csr!(scause, set_scause, "scause");
rw_csr!(stval, set_stval, "stval");
rw_csr!(satp, set_satp, "satp");
rw_csr!(sie, set_sie, "sie");
rw_csr!(sip, set_sip, "sip");
rw_csr!(time, _set_time, "time");
rw_csr!(scounteren, set_scounteren, "scounteren");

#[inline]
#[allow(dead_code)]
pub fn sfence_vma_all() {
    unsafe { asm!("sfence.vma zero, zero", options(nostack, nomem)) };
}

#[inline]
#[allow(dead_code)]
pub fn sfence_vma_va(vaddr: usize) {
    unsafe { asm!("sfence.vma {0}, zero", in(reg) vaddr, options(nostack, nomem)) };
}

#[inline]
#[allow(dead_code)]
pub fn sfence_vma_asid(asid: usize) {
    unsafe { asm!("sfence.vma zero, {0}", in(reg) asid, options(nostack, nomem)) };
}

#[inline]
#[allow(dead_code)]
pub fn fence_i() {
    unsafe { asm!("fence.i", options(nostack, nomem)) };
}
