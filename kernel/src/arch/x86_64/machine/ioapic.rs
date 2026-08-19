//! QEMU pc IOAPIC at the standard MMIO window.
//!
//! IRQ numbers follow seL4 pc99: user GetIOAPIC vectors are
//! `irq = arg + irq_user_min`, hardware IDT vector is `irq + IRQ_INT_OFFSET`.

use crate::arch::x86_64::object::vspace::paddr_to_pptr;

const IOAPIC_PADDR: usize = 0xfec0_0000;
const IOREGSEL: usize = 0x00;
const IOWIN: usize = 0x10;
const IOAPICVER: u32 = 0x01;
const IOREDTBL: u32 = 0x10;
const RTE_MASKED: u64 = 1 << 16;
const RTE_LEVEL: u64 = 1 << 15;
const RTE_POLARITY_LOW: u64 = 1 << 13;
const DEFAULT_PINS: usize = 24;
const MAX_IRQ: usize = 256;

/// seL4 `IRQ_INT_OFFSET`.
pub const IRQ_INT_OFFSET: u64 = 0x20;
/// seL4 `irq_user_min` (ISA 0..15 reserved when IOAPIC is used).
pub const IRQ_USER_MIN: u64 = 16;
/// seL4 `irq_user_max` (`int_irq_user_max - IRQ_INT_OFFSET`).
pub const IRQ_USER_MAX: u64 = 123;

#[derive(Copy, Clone)]
struct IrqRoute {
    mapped: bool,
    pin: u8,
}

const EMPTY_ROUTE: IrqRoute = IrqRoute {
    mapped: false,
    pin: 0,
};

static mut PIN_COUNT: usize = DEFAULT_PINS;
static mut IRQ_ROUTES: [IrqRoute; MAX_IRQ + 1] = [EMPTY_ROUTE; MAX_IRQ + 1];

fn mmio() -> *mut u8 {
    paddr_to_pptr(IOAPIC_PADDR) as *mut u8
}

unsafe fn write_sel(reg: u32) {
    unsafe {
        core::ptr::write_volatile(mmio().add(IOREGSEL) as *mut u32, reg);
    }
}

unsafe fn read_win() -> u32 {
    unsafe { core::ptr::read_volatile(mmio().add(IOWIN) as *const u32) }
}

unsafe fn write_win(value: u32) {
    unsafe {
        core::ptr::write_volatile(mmio().add(IOWIN) as *mut u32, value);
    }
}

unsafe fn read_rte(pin: u32) -> u64 {
    unsafe {
        write_sel(IOREDTBL + pin * 2);
        let low = read_win() as u64;
        write_sel(IOREDTBL + pin * 2 + 1);
        let high = read_win() as u64;
        (high << 32) | low
    }
}

unsafe fn write_rte(pin: u32, value: u64) {
    unsafe {
        write_sel(IOREDTBL + pin * 2);
        write_win(value as u32);
        write_sel(IOREDTBL + pin * 2 + 1);
        write_win((value >> 32) as u32);
    }
}

fn dest_field() -> u64 {
    (super::lapic::local_apic_id() as u64) << 56
}

pub fn init() {
    let ver = unsafe {
        write_sel(IOAPICVER);
        read_win()
    };
    let pins = ((ver >> 16) & 0xff) as usize + 1;
    let pins = pins.min(DEFAULT_PINS).max(1);
    unsafe {
        PIN_COUNT = pins;
        IRQ_ROUTES = [EMPTY_ROUTE; MAX_IRQ + 1];
        let dest = dest_field();
        let mut pin = 0u32;
        while pin < pins as u32 {
            write_rte(pin, dest | RTE_MASKED);
            pin += 1;
        }
    }
}

pub fn pin_count() -> usize {
    unsafe { PIN_COUNT }
}

pub fn map_pin_to_irq(pin: u64, irq: u64, level: bool, polarity_low: bool, vector: u8) {
    if irq > MAX_IRQ as u64 || pin >= pin_count() as u64 {
        return;
    }
    let mut rte = dest_field() | RTE_MASKED | u64::from(vector);
    if level {
        rte |= RTE_LEVEL;
    }
    if polarity_low {
        rte |= RTE_POLARITY_LOW;
    }
    unsafe {
        write_rte(pin as u32, rte);
        IRQ_ROUTES[irq as usize] = IrqRoute {
            mapped: true,
            pin: pin as u8,
        };
    }
}

pub fn irq_is_mapped(irq: u64) -> bool {
    if irq > MAX_IRQ as u64 {
        return false;
    }
    unsafe { IRQ_ROUTES[irq as usize].mapped }
}

pub fn irq_to_vector(irq: u64) -> Option<u8> {
    if irq_is_mapped(irq) {
        Some((irq + IRQ_INT_OFFSET) as u8)
    } else {
        None
    }
}

pub fn vector_to_irq(vector: u64) -> Option<u64> {
    if vector < IRQ_INT_OFFSET {
        return None;
    }
    let irq = vector - IRQ_INT_OFFSET;
    irq_is_mapped(irq).then_some(irq)
}

pub fn set_pin_masked(irq: u64, masked: bool) {
    if irq > MAX_IRQ as u64 {
        return;
    }
    let pin = unsafe {
        let route = core::ptr::addr_of!(IRQ_ROUTES[irq as usize]).read();
        route.mapped.then_some(route.pin)
    };
    let Some(pin) = pin else {
        return;
    };
    unsafe {
        let mut rte = read_rte(u32::from(pin));
        if masked {
            rte |= RTE_MASKED;
        } else {
            rte &= !RTE_MASKED;
        }
        write_rte(u32::from(pin), rte);
    }
}
