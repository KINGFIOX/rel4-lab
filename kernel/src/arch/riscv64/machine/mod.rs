pub mod csr;
pub mod fpu;
pub mod irq;
pub mod paging;

#[inline]
pub fn current_scratch() -> usize {
    csr::sscratch()
}

#[inline]
pub fn set_current_scratch(scratch: usize) {
    csr::set_sscratch(scratch);
}

#[inline]
pub fn full_memory_barrier() {
    core::sync::atomic::fence(core::sync::atomic::Ordering::SeqCst);
}

#[inline]
pub fn tlb_flush_all() {
    csr::sfence_vma_all();
}

#[inline]
pub fn tlb_flush_asid(asid: usize) {
    csr::sfence_vma_asid(asid);
}

#[inline]
pub fn tlb_flush_vaddr(vaddr: usize) {
    csr::sfence_vma_va(vaddr);
}
