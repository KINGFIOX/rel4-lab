//! LoongArch64 FPU hooks.
//!
//! Kernel FPU context management remains out of scope for the current
//! LoongArch staging backend. These hooks preserve the shared scheduler and
//! TCB interfaces while leaving user FPU access disabled by policy.

use crate::object::tcb::{self, Tcb};

pub fn init_current_core() {}

pub fn clear_supervisor_access() {}

pub fn disable_access() {}

pub fn lazy_restore(thread: *mut Tcb) {
    if thread.is_null() {
        return;
    }
    unsafe {
        tcb::set_fpu_context_enabled(thread, false);
    }
}

pub fn release(_thread: *mut Tcb) {}

pub fn release_on_current_core(_thread: *mut Tcb) {}
