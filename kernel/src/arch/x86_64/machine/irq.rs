pub const IOAPIC_MAX_IRQ: usize = 255;
pub const KERNEL_TIMER_IRQ: usize = IOAPIC_MAX_IRQ + 1;
pub const MAX_IRQ: usize = KERNEL_TIMER_IRQ;

pub fn init() {}

pub fn init_current_core() {}

pub fn local_irq_save() -> bool {
    false
}

pub fn local_irq_restore(_irq_was_enabled: bool) {}

pub fn is_external_irq(irq: u64) -> bool {
    irq <= IOAPIC_MAX_IRQ as u64
}

pub fn enable_external_irq(_irq: u64) {}

pub fn disable_external_irq(_irq: u64) {}

pub fn claim_external_irq() -> Option<u64> {
    None
}

pub fn complete_external_irq(_irq: u64) {}
