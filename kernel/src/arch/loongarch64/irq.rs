const CRMD_IE: usize = 1 << 2;
const ECFG_LIE_EXTIOI0: usize = 1 << 2;

pub const MAX_IRQ: usize = 256;
pub const KERNEL_TIMER_IRQ: usize = MAX_IRQ;

pub fn init() {
    crate::machine::loongarch_irq::init();
    super::csr::set_ecfg(super::csr::ecfg() | ECFG_LIE_EXTIOI0);
}

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
    crate::machine::loongarch_irq::is_external_irq(irq)
}

#[inline]
pub fn enable_external_irq(irq: u64) {
    crate::machine::loongarch_irq::enable_irq(irq);
}

#[inline]
pub fn disable_external_irq(irq: u64) {
    crate::machine::loongarch_irq::disable_irq(irq);
}

#[inline]
pub fn claim_external_irq() -> Option<u64> {
    crate::machine::loongarch_irq::claim()
}

#[inline]
pub fn complete_external_irq(irq: u64) {
    crate::machine::loongarch_irq::complete(irq);
}
