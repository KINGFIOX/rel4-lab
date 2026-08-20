//! Local TLB invalidation via `sfence.vma`.

// This module is written entirely in terms of the safe abstractions in
// `ktypes`; keep it that way.
#![deny(unsafe_code)]

use super::csr;

pub fn flush_all() {
    csr::sfence_vma_all();
}

pub fn flush_asid(asid: usize) {
    csr::sfence_vma_asid(asid);
}

pub fn flush_vaddr(vaddr: usize) {
    csr::sfence_vma_va(vaddr);
}
