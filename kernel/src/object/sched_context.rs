//! Disabled scheduling-context compatibility surface.
//!
//! This kernel intentionally does not implement seL4 MCS real-time scheduling.
//! Sched-context objects may still be created by existing userland and boot
//! code, but they do not carry budget, refill, donation, timeout, or yield-to
//! scheduler state.

use crate::object::tcb::Tcb;

pub unsafe fn init(sc_kva: u64, _core: u64) {
    crate::kernel::smp::debug_assert_kernel_lock_held();
    if sc_kva == 0 {
        return;
    }
    unsafe {
        core::ptr::write_bytes(
            sc_kva as *mut u8,
            0,
            crate::abi::constants::SEL4_CORE_SCHED_CONTEXT_BYTES as usize,
        );
    }
}

pub unsafe fn configure(
    _sc_kva: u64,
    _budget: u64,
    _period: u64,
    _extra_refills: u64,
    _badge: u64,
    _flags: u64,
) {
}

pub unsafe fn yield_tcb(_tcb: *mut Tcb) -> bool {
    false
}

pub unsafe fn set_reply_head(_sc_kva: u64, _reply_kva: u64) {}

pub unsafe fn clear_reply_head(_sc_kva: u64) {}

pub unsafe fn finalize(_sc_kva: u64) {
    crate::kernel::smp::debug_assert_kernel_lock_held();
}
