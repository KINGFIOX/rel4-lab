use core::ptr;

use crate::arch::loongarch64::machine::csr;
use crate::arch::loongarch64::object::vspace::paddr_to_mmio;
use crate::arch::loongarch64::plat as platform;

const PCH_PIC_INT_MASK: usize = 0x20;
const PCH_PIC_HTMSI_VEC: usize = 0x200;
const PCH_PIC_INT_CLEAR: usize = 0x80;

const PCH_PIC_IRQ_NUM: usize = 32;
const EXTIOI_IRQS: usize = 256;
const EXTIOI_GROUP_BITS: usize = 32;
const EXTIOI_GROUPS: usize = EXTIOI_IRQS / EXTIOI_GROUP_BITS;

const EXTIOI_BASE: usize = 0x1400;
const EXTIOI_IPMAP_START: usize = EXTIOI_BASE + 0x0c0;
const EXTIOI_ENABLE_START: usize = EXTIOI_BASE + 0x200;
const EXTIOI_COREISR_START: usize = EXTIOI_BASE + 0x400;
const EXTIOI_COREMAP_START: usize = EXTIOI_BASE + 0x800;

const CPU0_BITMAP_PER_BYTE: u32 = 0x0101_0101;
const EXTIOI_CPU_IP0_PER_BYTE: u32 = 0x0101_0101;

fn pch_reg8(offset: usize) -> *mut u8 {
    paddr_to_mmio(platform::PCH_PIC_BASE_PA + offset) as *mut u8
}

fn pch_reg64(offset: usize) -> *mut u64 {
    paddr_to_mmio(platform::PCH_PIC_BASE_PA + offset) as *mut u64
}

pub fn init() {
    unsafe {
        ptr::write_volatile(pch_reg64(PCH_PIC_INT_MASK), u64::MAX);
        ptr::write_volatile(pch_reg64(PCH_PIC_INT_CLEAR), u64::MAX);
        for irq in 0..PCH_PIC_IRQ_NUM {
            ptr::write_volatile(pch_reg8(PCH_PIC_HTMSI_VEC + irq), irq as u8);
        }
    }

    for group in 0..EXTIOI_GROUPS {
        let offset = group * 4;
        csr::iocsr_write32(EXTIOI_ENABLE_START + offset, 0);
        csr::iocsr_write32(EXTIOI_COREISR_START + offset, u32::MAX);
    }
    for index in 0..(EXTIOI_GROUPS / 4) {
        csr::iocsr_write32(EXTIOI_IPMAP_START + index * 4, EXTIOI_CPU_IP0_PER_BYTE);
    }
    for index in 0..(EXTIOI_IRQS / 4) {
        csr::iocsr_write32(EXTIOI_COREMAP_START + index * 4, CPU0_BITMAP_PER_BYTE);
    }
    csr::dbar();
}

#[inline]
pub fn is_external_irq(irq: u64) -> bool {
    irq > 0 && irq < EXTIOI_IRQS as u64
}

pub fn enable_irq(irq: u64) {
    if !is_external_irq(irq) {
        return;
    }

    let irq = irq as usize;
    let group = irq / EXTIOI_GROUP_BITS;
    let mask = 1u32 << (irq % EXTIOI_GROUP_BITS);
    let enable_addr = EXTIOI_ENABLE_START + group * 4;
    csr::iocsr_write32(EXTIOI_COREISR_START + group * 4, mask);
    if irq < PCH_PIC_IRQ_NUM {
        unsafe {
            ptr::write_volatile(pch_reg64(PCH_PIC_INT_CLEAR), 1u64 << irq);
        }
    }
    csr::iocsr_write32(enable_addr, csr::iocsr_read32(enable_addr) | mask);

    if irq < PCH_PIC_IRQ_NUM {
        unsafe {
            let mask_reg = pch_reg64(PCH_PIC_INT_MASK);
            ptr::write_volatile(mask_reg, ptr::read_volatile(mask_reg) & !(1u64 << irq));
        }
    }
    csr::dbar();
}

pub fn disable_irq(irq: u64) {
    if !is_external_irq(irq) {
        return;
    }

    let irq = irq as usize;
    if irq < PCH_PIC_IRQ_NUM {
        unsafe {
            let mask_reg = pch_reg64(PCH_PIC_INT_MASK);
            ptr::write_volatile(mask_reg, ptr::read_volatile(mask_reg) | (1u64 << irq));
        }
    }

    let group = irq / EXTIOI_GROUP_BITS;
    let mask = 1u32 << (irq % EXTIOI_GROUP_BITS);
    let enable_addr = EXTIOI_ENABLE_START + group * 4;
    csr::iocsr_write32(enable_addr, csr::iocsr_read32(enable_addr) & !mask);
    csr::dbar();
}

pub fn claim() -> Option<u64> {
    for group in 0..EXTIOI_GROUPS {
        let pending = csr::iocsr_read32(EXTIOI_COREISR_START + group * 4);
        let pending = if group == 0 { pending & !1 } else { pending };
        if pending != 0 {
            let bit = pending.trailing_zeros() as usize;
            return Some((group * EXTIOI_GROUP_BITS + bit) as u64);
        }
    }
    None
}

pub fn complete(irq: u64) {
    if !is_external_irq(irq) {
        return;
    }

    let irq = irq as usize;
    let group = irq / EXTIOI_GROUP_BITS;
    let mask = 1u32 << (irq % EXTIOI_GROUP_BITS);
    csr::iocsr_write32(EXTIOI_COREISR_START + group * 4, mask);
    if irq < PCH_PIC_IRQ_NUM {
        unsafe {
            ptr::write_volatile(pch_reg64(PCH_PIC_INT_CLEAR), 1u64 << irq);
        }
    }
    csr::dbar();
}
