pub mod ipi;

pub const SUPPORTS_REMOTE_IPI: bool = ipi::SUPPORTS_REMOTE_IPI;
pub const SUPPORTS_REMOTE_TLB_FLUSH: bool = ipi::SUPPORTS_REMOTE_TLB_FLUSH;

#[inline]
pub fn send_ipi(cpu_id: usize) -> isize {
    ipi::send_ipi(1, cpu_id).error
}

#[inline]
pub fn remote_tlb_flush_all(_cpu_id: usize) -> isize {
    0
}

#[inline]
pub fn remote_tlb_flush_asid(_cpu_id: usize, _asid: usize) -> isize {
    0
}

#[inline]
pub fn complete_remote_call() {}
