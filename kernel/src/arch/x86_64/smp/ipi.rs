//! x2APIC IPI and remote TLB shootdown.

use core::sync::atomic::{AtomicU64, Ordering};

use crate::abi::constants::MAX_NUM_NODES;
use crate::arch::x86_64::machine::{lapic, tlb};

pub const SUPPORTS_REMOTE_IPI: bool = true;
pub const SUPPORTS_REMOTE_TLB_FLUSH: bool = true;

#[derive(Copy, Clone, Debug, Eq, PartialEq)]
pub struct IpiRet {
    pub error: isize,
    pub value: usize,
}

const OK: IpiRet = IpiRet { error: 0, value: 0 };

static TLB_GEN: AtomicU64 = AtomicU64::new(1);
static TLB_DONE: [AtomicU64; 8] = [const { AtomicU64::new(0) }; 8];

fn dest_apic_id(cpu_id: usize) -> u32 {
    cpu_id as u32
}

fn core_index(cpu_id: usize) -> usize {
    cpu_id.min(TLB_DONE.len().saturating_sub(1))
}

pub fn send_ipi(_mask: usize, cpu_id: usize) -> IpiRet {
    lapic::send_ipi(dest_apic_id(cpu_id), lapic::IPI_VECTOR);
    OK
}

pub fn remote_tlb_flush(_mask: usize, cpu_id: usize, _start: usize, _size: usize) -> IpiRet {
    shootdown(cpu_id)
}

pub fn remote_tlb_flush_asid(
    mask: usize,
    cpu_id: usize,
    start: usize,
    size: usize,
    _asid: usize,
) -> IpiRet {
    remote_tlb_flush(mask, cpu_id, start, size)
}

pub fn handle_ipi() {
    tlb::flush_all();
    let core = crate::kernel::smp::current_core_id().min(TLB_DONE.len().saturating_sub(1));
    TLB_DONE[core].store(TLB_GEN.load(Ordering::Acquire), Ordering::Release);
    lapic::eoi();
}

fn shootdown(cpu_id: usize) -> IpiRet {
    let token = TLB_GEN.fetch_add(1, Ordering::AcqRel);
    let core = core_index(cpu_id);
    lapic::send_ipi(dest_apic_id(cpu_id), lapic::IPI_VECTOR);
    while TLB_DONE[core].load(Ordering::Acquire) < token {
        core::hint::spin_loop();
    }
    let _ = MAX_NUM_NODES;
    OK
}

pub fn ack_ipi() {}
