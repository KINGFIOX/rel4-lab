//! Local APIC timer delivery. IOAPIC and IPI are out of scope.

pub const IOAPIC_MAX_IRQ: usize = 255;
pub const KERNEL_TIMER_IRQ: usize = IOAPIC_MAX_IRQ + 1;
pub const MAX_IRQ: usize = KERNEL_TIMER_IRQ;

pub fn init() {
    mask_legacy_pic();
    super::lapic::init();
}

fn mask_legacy_pic() {
    unsafe {
        core::arch::asm!(
            "mov $0xff, %al",
            "out %al, $0xa1",
            "out %al, $0x21",
            out("al") _,
            options(att_syntax, nomem, nostack, preserves_flags),
        );
    }
}

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

pub fn complete_external_irq(_irq: u64) {
    super::lapic::eoi();
}
