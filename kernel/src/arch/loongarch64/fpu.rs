//! LoongArch64 FPU hooks.
//!
//! Kernel FPU context management remains out of scope for the current
//! LoongArch staging backend. These hooks preserve the shared scheduler and
//! TCB interfaces while leaving user FPU access disabled by policy.

use crate::object::tcb::{self, Tcb};

const EUEN_FPE: usize = 1 << 0;
const EUEN_SXE: usize = 1 << 1;
const EUEN_ASXE: usize = 1 << 2;
const EUEN_FPU_STATE_MASK: usize = EUEN_FPE | EUEN_SXE | EUEN_ASXE;
pub(crate) const EUEN_FPU_STATE_CLEAR_MASK: i64 = !(EUEN_FPU_STATE_MASK as i64);

#[inline]
fn clear_fpu_enable() {
    let euen = crate::arch::loongarch64::csr::euen();
    crate::arch::loongarch64::csr::set_euen(euen & !EUEN_FPU_STATE_MASK);
    crate::arch::loongarch64::csr::dbar();
}

pub fn init_current_core() {
    clear_fpu_enable();
}

pub fn clear_supervisor_access() {
    clear_fpu_enable();
}

pub fn disable_access() {
    clear_fpu_enable();
}

pub fn lazy_restore(thread: *mut Tcb) {
    if thread.is_null() {
        return;
    }
    disable_access();
    unsafe {
        tcb::set_fpu_context_enabled(thread, false);
    }
}

pub fn release(_thread: *mut Tcb) {}

pub fn release_on_current_core(_thread: *mut Tcb) {}
