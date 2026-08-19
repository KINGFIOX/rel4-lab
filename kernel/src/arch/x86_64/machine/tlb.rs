//! Local TLB invalidation for the x86_64 backend.

use super::registers;

pub fn flush_all() {
    registers::flush_tlb();
}

pub fn flush_asid(_asid: usize) {
    registers::flush_tlb();
}

pub fn flush_vaddr(vaddr: usize) {
    registers::invlpg(vaddr);
}
