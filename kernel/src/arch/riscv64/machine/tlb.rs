//! Local TLB invalidation via `sfence.vma`.

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
