const CRMD_IE: usize = 1 << 2;

pub const MAX_IRQ: usize = 255;
pub const KERNEL_TIMER_IRQ: usize = MAX_IRQ;

pub fn init() {}

#[inline]
pub fn local_irq_save() -> bool {
    let crmd = super::csr::crmd();
    let irq_was_enabled = (crmd & CRMD_IE) != 0;
    if irq_was_enabled {
        super::csr::set_crmd(crmd & !CRMD_IE);
    }
    irq_was_enabled
}

#[inline]
pub fn local_irq_restore(irq_was_enabled: bool) {
    let crmd = super::csr::crmd();
    if irq_was_enabled {
        super::csr::set_crmd(crmd | CRMD_IE);
    } else {
        super::csr::set_crmd(crmd & !CRMD_IE);
    }
}

#[inline]
pub fn is_external_irq(irq: u64) -> bool {
    irq > 0 && irq < KERNEL_TIMER_IRQ as u64
}

#[inline]
pub fn enable_external_irq(_irq: u64) {}

#[inline]
pub fn disable_external_irq(_irq: u64) {}

#[inline]
pub fn claim_external_irq() -> Option<u64> {
    None
}

#[inline]
pub fn complete_external_irq(_irq: u64) {}
