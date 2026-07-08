pub mod ipi;

pub const SUPPORTS_REMOTE_IPI: bool = ipi::SUPPORTS_REMOTE_IPI;
pub const SUPPORTS_REMOTE_TLB_FLUSH: bool = ipi::SUPPORTS_REMOTE_TLB_FLUSH;

#[inline]
pub fn send_ipi(hart_id: usize) -> isize {
    ipi::send_ipi(1, hart_id).error
}

#[inline]
pub fn remote_tlb_flush_all(hart_id: usize) -> isize {
    ipi::remote_sfence_vma(1, hart_id, 0, 0).error
}

#[inline]
pub fn remote_tlb_flush_asid(hart_id: usize, asid: usize) -> isize {
    ipi::remote_sfence_vma_asid(1, hart_id, 0, 0, asid).error
}

#[inline]
pub fn complete_remote_call() {}
