pub mod fpu;
pub mod irq;
pub mod paging;
pub mod registers;

#[inline]
pub fn current_scratch() -> usize {
    registers::sscratch()
}

#[inline]
pub fn set_current_scratch(scratch: usize) {
    registers::set_sscratch(scratch);
}

#[inline]
pub fn full_memory_barrier() {
    core::sync::atomic::fence(core::sync::atomic::Ordering::SeqCst);
}

#[inline]
pub fn tlb_flush_all() {
    registers::sfence_vma_all();
}

#[inline]
pub fn tlb_flush_asid(asid: usize) {
    registers::sfence_vma_asid(asid);
}

#[inline]
pub fn tlb_flush_vaddr(vaddr: usize) {
    registers::sfence_vma_va(vaddr);
}
