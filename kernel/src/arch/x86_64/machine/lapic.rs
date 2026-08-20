//! x2APIC timer and IPI delivery.

use crate::arch::x86_64::machine::registers::{self, IA32_APIC_BASE};

const APIC_ENABLE: u64 = 1 << 11;
const APIC_X2APIC: u64 = 1 << 10;

const MSR_APIC_ID: u32 = 0x802;
const MSR_EOI: u32 = 0x80b;
const MSR_SVR: u32 = 0x80f;
const MSR_ICR: u32 = 0x830;
const MSR_IRR1: u32 = 0x821;
const MSR_LVT_TIMER: u32 = 0x832;
const MSR_TIMER_INIT: u32 = 0x838;
const MSR_TIMER_DIV: u32 = 0x83e;

pub const TIMER_VECTOR: u8 = 32;
pub const IPI_VECTOR: u8 = 0xfd;
const SVR_APIC_ENABLE: u64 = 1 << 8;
const LVT_PERIODIC: u64 = 1 << 17;
const DIVIDE_BY_1: u64 = 0xb;
const TIMER_INIT_COUNT: u64 = 10_000_000;
const DELIVERY_FIXED: u64 = 0 << 8;
const DELIVERY_INIT: u64 = 5 << 8;
const DELIVERY_SIPI: u64 = 6 << 8;
const LEVEL_ASSERT: u64 = 1 << 14;
const TRIGGER_LEVEL: u64 = 1 << 15;

pub fn init() {
    let mut apic_base = registers::rdmsr(IA32_APIC_BASE);
    apic_base |= APIC_ENABLE;
    // SAFETY: these are the local APIC's own MSRs, written with the enable,
    // x2APIC, spurious-vector, and periodic-timer values this platform uses.
    unsafe {
        registers::wrmsr(IA32_APIC_BASE, apic_base);
        registers::wrmsr(IA32_APIC_BASE, apic_base | APIC_X2APIC);
        registers::wrmsr(MSR_SVR, SVR_APIC_ENABLE | 0xff);
        registers::wrmsr(MSR_TIMER_DIV, DIVIDE_BY_1);
        registers::wrmsr(MSR_LVT_TIMER, LVT_PERIODIC | u64::from(TIMER_VECTOR));
        registers::wrmsr(MSR_TIMER_INIT, TIMER_INIT_COUNT);
    }
}

pub fn eoi() {
    // SAFETY: writing the local APIC's end-of-interrupt register.
    unsafe { registers::wrmsr(MSR_EOI, 0) };
}

pub fn timer_irq_pending() -> bool {
    (registers::rdmsr(MSR_IRR1) & (1 << (u32::from(TIMER_VECTOR) % 32))) != 0
}

pub fn local_apic_id() -> u32 {
    registers::rdmsr(MSR_APIC_ID) as u32
}

pub fn send_ipi(dest_apic_id: u32, vector: u8) {
    write_icr((u64::from(dest_apic_id) << 32) | DELIVERY_FIXED | u64::from(vector));
}

pub fn send_init(dest_apic_id: u32) {
    write_icr((u64::from(dest_apic_id) << 32) | DELIVERY_INIT | LEVEL_ASSERT | TRIGGER_LEVEL);
    write_icr((u64::from(dest_apic_id) << 32) | DELIVERY_INIT | TRIGGER_LEVEL);
}

pub fn send_sipi(dest_apic_id: u32, vector: u8) {
    write_icr((u64::from(dest_apic_id) << 32) | DELIVERY_SIPI | u64::from(vector));
}

fn write_icr(value: u64) {
    // SAFETY: writing the local APIC's interrupt command register, whose
    // effect is to send the IPI the callers above encoded.
    unsafe { registers::wrmsr(MSR_ICR, value) };
}
