//! QEMU pc IOAPIC at the standard MMIO window.
//!
//! IRQ numbers follow seL4 pc99: user GetIOAPIC vectors are
//! `irq = arg + irq_user_min`, hardware IDT vector is `irq + IRQ_INT_OFFSET`.

use crate::arch::x86_64::object::vspace::paddr_to_pptr;
use crate::kernel::smp::BklCell;
use crate::ktypes::addr::Paddr;
use crate::ktypes::mmio::MmioRegion;

const IOAPIC_PADDR: usize = 0xfec0_0000;
const IOAPIC_BYTES: usize = 0x20;
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

/// How many redirection-table pins this IOAPIC reported, and which pin each
/// IRQ was routed to. Written while routing IRQs, so it is BKL-protected like
/// the rest of the kernel's interrupt bookkeeping.
struct Routing {
    pin_count: usize,
    routes: [IrqRoute; MAX_IRQ + 1],
}

static ROUTING: BklCell<Routing> = BklCell::new(Routing {
    pin_count: DEFAULT_PINS,
    routes: [EMPTY_ROUTE; MAX_IRQ + 1],
});

/// The IOAPIC's two-register window.
fn ioapic() -> MmioRegion {
    // SAFETY: the platform places the IOAPIC at this fixed physical address,
    // the kernel window maps it, and no Rust object lives there.
    unsafe { MmioRegion::new(paddr_to_pptr(Paddr::new(IOAPIC_PADDR)), IOAPIC_BYTES) }
}

/// Select an indirect register. Every access is a select followed by a window
/// read or write, so the two must not be interleaved with another core's
/// access; the BKL provides that.
fn write_sel(reg: u32) {
    ioapic().reg::<u32>(IOREGSEL).write(reg);
}

fn read_win() -> u32 {
    ioapic().reg::<u32>(IOWIN).read()
}

fn write_win(value: u32) {
    ioapic().reg::<u32>(IOWIN).write(value);
}

/// Read a redirection-table entry, which spans two indirect registers.
fn read_rte(pin: u32) -> u64 {
    write_sel(IOREDTBL + pin * 2);
    let low = u64::from(read_win());
    write_sel(IOREDTBL + pin * 2 + 1);
    let high = u64::from(read_win());
    (high << 32) | low
}

fn write_rte(pin: u32, value: u64) {
    write_sel(IOREDTBL + pin * 2);
    write_win(value as u32);
    write_sel(IOREDTBL + pin * 2 + 1);
    write_win((value >> 32) as u32);
}

fn dest_field() -> u64 {
    (super::lapic::local_apic_id() as u64) << 56
}

pub fn init() {
    write_sel(IOAPICVER);
    let ver = read_win();
    let pins = (((ver >> 16) & 0xff) as usize + 1).clamp(1, DEFAULT_PINS);
    ROUTING.with_mut(|routing| {
        routing.pin_count = pins;
        routing.routes = [EMPTY_ROUTE; MAX_IRQ + 1];
    });
    let dest = dest_field();
    for pin in 0..pins as u32 {
        write_rte(pin, dest | RTE_MASKED);
    }
}

pub fn pin_count() -> usize {
    ROUTING.with_ref(|routing| routing.pin_count)
}

/// The pin an IRQ is routed to, if it is routed at all.
fn route_pin(irq: u64) -> Option<u8> {
    ROUTING.with_ref(|routing| {
        let route = routing.routes.get(irq as usize)?;
        route.mapped.then_some(route.pin)
    })
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
    write_rte(pin as u32, rte);
    ROUTING.with_mut(|routing| {
        routing.routes[irq as usize] = IrqRoute {
            mapped: true,
            pin: pin as u8,
        };
    });
}

pub fn irq_is_mapped(irq: u64) -> bool {
    route_pin(irq).is_some()
}

pub fn irq_to_vector(irq: u64) -> Option<u8> {
    irq_is_mapped(irq).then(|| (irq + IRQ_INT_OFFSET) as u8)
}

pub fn vector_to_irq(vector: u64) -> Option<u64> {
    if vector < IRQ_INT_OFFSET {
        return None;
    }
    let irq = vector - IRQ_INT_OFFSET;
    irq_is_mapped(irq).then_some(irq)
}

pub fn set_pin_masked(irq: u64, masked: bool) {
    let Some(pin) = route_pin(irq) else {
        return;
    };
    let mut rte = read_rte(u32::from(pin));
    if masked {
        rte |= RTE_MASKED;
    } else {
        rte &= !RTE_MASKED;
    }
    write_rte(u32::from(pin), rte);
}
