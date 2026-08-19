use core::ptr;

use crate::abi::constants::PPTR_BASE;

const PLIC_BASE_PA: usize = 0x0c00_0000;
const S_CONTEXT: usize = 1;

const PRIORITY_BASE: usize = 0x0000;
const ENABLE_BASE: usize = 0x2000 + S_CONTEXT * 0x80;
const THRESHOLD_BASE: usize = 0x20_0000 + S_CONTEXT * 0x1000;
const CLAIM_COMPLETE: usize = THRESHOLD_BASE + 4;

fn reg32(offset: usize) -> *mut u32 {
    (PPTR_BASE + PLIC_BASE_PA + offset) as *mut u32
}

pub fn init() {
    unsafe {
        ptr::write_volatile(reg32(THRESHOLD_BASE), 0);
    }
}

pub fn enable_irq(irq: usize) {
    if irq == 0 {
        return;
    }
    unsafe {
        ptr::write_volatile(reg32(PRIORITY_BASE + irq * 4), 1);
        let enable = reg32(ENABLE_BASE + (irq / 32) * 4);
        let mask = 1u32 << (irq % 32);
        ptr::write_volatile(enable, ptr::read_volatile(enable) | mask);
    }
}

pub fn disable_irq(irq: usize) {
    if irq == 0 {
        return;
    }
    unsafe {
        let enable = reg32(ENABLE_BASE + (irq / 32) * 4);
        let mask = 1u32 << (irq % 32);
        ptr::write_volatile(enable, ptr::read_volatile(enable) & !mask);
    }
}

pub fn claim() -> u32 {
    unsafe { ptr::read_volatile(reg32(CLAIM_COMPLETE)) }
}

pub fn complete(irq: u32) {
    if irq != 0 {
        unsafe {
            ptr::write_volatile(reg32(CLAIM_COMPLETE), irq);
        }
    }
}
