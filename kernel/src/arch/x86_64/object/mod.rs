pub mod vspace;

pub mod interrupt {
    pub use crate::arch::x86_64::machine::irq::{
        KERNEL_TIMER_IRQ, MAX_IRQ, complete_external_irq, disable_external_irq, enable_external_irq,
    };
}

pub const ASID_BITS: usize = 16;
