pub const PLIC_MAX_IRQ: usize = 95;
pub const KERNEL_TIMER_IRQ: usize = PLIC_MAX_IRQ + 1;
pub const MAX_IRQ: usize = KERNEL_TIMER_IRQ;

pub fn init() {
    crate::machine::plic::init();
}

#[inline]
pub fn is_external_irq(irq: u64) -> bool {
    irq > 0 && irq <= PLIC_MAX_IRQ as u64
}

#[inline]
pub fn enable_external_irq(irq: u64) {
    if is_external_irq(irq) {
        crate::machine::plic::enable_irq(irq as usize);
    }
}

#[inline]
pub fn disable_external_irq(irq: u64) {
    if is_external_irq(irq) {
        crate::machine::plic::disable_irq(irq as usize);
    }
}

#[inline]
pub fn claim_external_irq() -> Option<u64> {
    let irq = crate::machine::plic::claim();
    if irq == 0 { None } else { Some(irq as u64) }
}

#[inline]
pub fn complete_external_irq(irq: u64) {
    if is_external_irq(irq) {
        crate::machine::plic::complete(irq as u32);
    }
}
