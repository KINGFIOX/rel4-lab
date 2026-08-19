//! x86_64 machine hooks: paging, LAPIC/IOAPIC placeholders, and TLB.

pub mod fpu;
pub mod irq;
pub mod paging;
pub mod registers;
pub mod tlb;

#[inline]
pub fn current_scratch() -> usize {
    registers::current_scratch()
}

#[inline]
pub fn set_current_scratch(scratch: usize) {
    registers::set_current_scratch(scratch);
}

#[inline]
pub fn full_memory_barrier() {
    core::sync::atomic::fence(core::sync::atomic::Ordering::SeqCst);
}

#[inline]
pub fn tlb_flush_all() {
    tlb::flush_all();
}

#[inline]
pub fn tlb_flush_asid(asid: usize) {
    tlb::flush_asid(asid);
}

#[inline]
pub fn tlb_flush_vaddr(vaddr: usize) {
    tlb::flush_vaddr(vaddr);
}
