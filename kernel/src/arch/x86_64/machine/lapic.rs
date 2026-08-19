//! Single-core x2APIC timer. No IOAPIC and no IPI.

use crate::arch::x86_64::machine::registers::{self, IA32_APIC_BASE};

const APIC_ENABLE: u64 = 1 << 11;
const APIC_X2APIC: u64 = 1 << 10;

const MSR_EOI: u32 = 0x80b;
const MSR_SVR: u32 = 0x80f;
const MSR_IRR1: u32 = 0x821;
const MSR_LVT_TIMER: u32 = 0x832;
const MSR_TIMER_INIT: u32 = 0x838;
const MSR_TIMER_DIV: u32 = 0x83e;

pub const TIMER_VECTOR: u8 = 32;
const SVR_APIC_ENABLE: u64 = 1 << 8;
const LVT_PERIODIC: u64 = 1 << 17;
const DIVIDE_BY_1: u64 = 0xb;
const TIMER_INIT_COUNT: u64 = 10_000_000;

pub fn init() {
    let mut apic_base = registers::rdmsr(IA32_APIC_BASE);
    apic_base |= APIC_ENABLE;
    registers::wrmsr(IA32_APIC_BASE, apic_base);
    registers::wrmsr(IA32_APIC_BASE, apic_base | APIC_X2APIC);
    registers::wrmsr(MSR_SVR, SVR_APIC_ENABLE | 0xff);
    registers::wrmsr(MSR_TIMER_DIV, DIVIDE_BY_1);
    registers::wrmsr(MSR_LVT_TIMER, LVT_PERIODIC | u64::from(TIMER_VECTOR));
    registers::wrmsr(MSR_TIMER_INIT, TIMER_INIT_COUNT);
}

pub fn eoi() {
    registers::wrmsr(MSR_EOI, 0);
}

pub fn timer_irq_pending() -> bool {
    (registers::rdmsr(MSR_IRR1) & (1 << (u32::from(TIMER_VECTOR) % 32))) != 0
}
