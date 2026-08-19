pub mod ipi;
pub mod trampoline;

use crate::abi::constants::MAX_NUM_NODES;

pub const SUPPORTS_REMOTE_IPI: bool = ipi::SUPPORTS_REMOTE_IPI;
pub const SUPPORTS_REMOTE_TLB_FLUSH: bool = ipi::SUPPORTS_REMOTE_TLB_FLUSH;

#[inline]
pub fn send_ipi(cpu_id: usize) -> isize {
    ipi::send_ipi(1, cpu_id).error
}

#[inline]
pub fn remote_tlb_flush_all(cpu_id: usize) -> isize {
    ipi::remote_tlb_flush(1, cpu_id, 0, 0).error
}

#[inline]
pub fn remote_tlb_flush_asid(cpu_id: usize, asid: usize) -> isize {
    ipi::remote_tlb_flush_asid(1, cpu_id, 0, 0, asid).error
}

#[inline]
pub fn complete_remote_call() {}

pub fn start_application_processors() {
    if MAX_NUM_NODES <= 1 {
        return;
    }
    trampoline::start_aps(MAX_NUM_NODES);
}
