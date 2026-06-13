const CRMD_IE: usize = 1 << 2;

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
