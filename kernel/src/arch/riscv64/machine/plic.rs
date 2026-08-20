//! QEMU virt PLIC, addressed through the kernel's MMIO window.

use crate::abi::constants::PPTR_BASE;
use crate::ktypes::addr::Kva;
use crate::ktypes::mmio::{MmioReg, MmioRegion};

const PLIC_BASE_PA: usize = 0x0c00_0000;
const PLIC_BYTES: usize = 0x400_0000;
const S_CONTEXT: usize = 1;

const PRIORITY_BASE: usize = 0x0000;
const ENABLE_BASE: usize = 0x2000 + S_CONTEXT * 0x80;
const THRESHOLD_BASE: usize = 0x20_0000 + S_CONTEXT * 0x1000;
const CLAIM_COMPLETE: usize = THRESHOLD_BASE + 4;

/// The PLIC's register window.
fn plic() -> MmioRegion {
    // SAFETY: the platform maps the PLIC at this fixed physical address, and
    // the kernel window covers it; no Rust object lives there.
    unsafe { MmioRegion::new(Kva::new(PPTR_BASE + PLIC_BASE_PA), PLIC_BYTES) }
}

fn reg32(offset: usize) -> MmioReg<u32> {
    plic().reg(offset)
}

/// Per-IRQ enable bit, as a register plus the bit within it.
fn enable_bit(irq: usize) -> (MmioReg<u32>, u32) {
    (reg32(ENABLE_BASE + (irq / 32) * 4), 1u32 << (irq % 32))
}

pub fn init() {
    reg32(THRESHOLD_BASE).write(0);
}

pub fn enable_irq(irq: usize) {
    if irq == 0 {
        return;
    }
    reg32(PRIORITY_BASE + irq * 4).write(1);
    let (enable, mask) = enable_bit(irq);
    enable.modify(|bits| bits | mask);
}

pub fn disable_irq(irq: usize) {
    if irq == 0 {
        return;
    }
    let (enable, mask) = enable_bit(irq);
    enable.modify(|bits| bits & !mask);
}

pub fn claim() -> u32 {
    reg32(CLAIM_COMPLETE).read()
}

pub fn complete(irq: u32) {
    if irq != 0 {
        reg32(CLAIM_COMPLETE).write(irq);
    }
}
