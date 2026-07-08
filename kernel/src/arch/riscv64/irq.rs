pub const PLIC_MAX_IRQ: usize = 95;
pub const KERNEL_TIMER_IRQ: usize = PLIC_MAX_IRQ + 1;
pub const MAX_IRQ: usize = KERNEL_TIMER_IRQ;

const SSTATUS_SIE: usize = 1 << 1;

pub fn init() {
    crate::machine::plic::init();
}

#[inline]
pub fn local_irq_save() -> bool {
    let sstatus = super::csr::sstatus();
    let irq_was_enabled = (sstatus & SSTATUS_SIE) != 0;
    if irq_was_enabled {
        super::csr::set_sstatus(sstatus & !SSTATUS_SIE);
    }
    irq_was_enabled
}

#[inline]
pub fn local_irq_restore(irq_was_enabled: bool) {
    let sstatus = super::csr::sstatus();
    if irq_was_enabled {
        super::csr::set_sstatus(sstatus | SSTATUS_SIE);
    } else {
        super::csr::set_sstatus(sstatus & !SSTATUS_SIE);
    }
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
