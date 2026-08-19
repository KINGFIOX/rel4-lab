//! x86_64 FPU ownership and lazy context switching.
//!
//! Hardware state is saved with `fxsave` / restored with `fxrstor`. `CR0.TS`
//! is set when the current user thread must not use the FPU; kernel entry
//! always clears TS so compiler-generated SSE in the kernel cannot #NM.

use core::arch::asm;
use core::ptr::null_mut;
use core::sync::atomic::{AtomicBool, AtomicUsize, Ordering};

use crate::abi::constants::MAX_NUM_NODES;
use crate::object::tcb::{self, Tcb};

const CR0_TS: usize = 1 << 3;

static FPU_OWNER: [AtomicUsize; MAX_NUM_NODES] = [const { AtomicUsize::new(0) }; MAX_NUM_NODES];
static FPU_ACCESS_ENABLED: [AtomicBool; MAX_NUM_NODES] =
    [const { AtomicBool::new(false) }; MAX_NUM_NODES];

#[inline]
fn core_index() -> usize {
    crate::kernel::smp::current_core_id().min(MAX_NUM_NODES.saturating_sub(1))
}

#[inline]
fn clts() {
    unsafe {
        asm!("clts", options(nostack, nomem, preserves_flags));
    }
}

#[inline]
fn set_ts() {
    unsafe {
        asm!(
            "mov {cr0}, cr0",
            "or {cr0}, {ts}",
            "mov cr0, {cr0}",
            cr0 = out(reg) _,
            ts = in(reg) CR0_TS,
            options(nostack, preserves_flags),
        );
    }
}

unsafe fn save_fpu_state(thread: *mut Tcb) {
    if thread.is_null() {
        return;
    }
    let dest = unsafe { (*thread).context.fpu.fxsave.as_mut_ptr() };
    clts();
    unsafe {
        asm!(
            "fxsave64 [{0}]",
            in(reg) dest,
            options(nostack, preserves_flags),
        );
    }
}

unsafe fn load_fpu_state(thread: *mut Tcb) {
    if thread.is_null() {
        return;
    }
    let src = unsafe { (*thread).context.fpu.fxsave.as_ptr() };
    clts();
    unsafe {
        asm!(
            "fxrstor64 [{0}]",
            in(reg) src,
            options(nostack, preserves_flags),
        );
    }
}

pub fn init_current_core() {
    let core = core_index();
    FPU_OWNER[core].store(0, Ordering::Release);
    clts();
    disable_access();
}

pub fn clear_supervisor_access() {
    clts();
}

#[inline]
pub fn disable_access() {
    FPU_ACCESS_ENABLED[core_index()].store(false, Ordering::Release);
    set_ts();
}

#[inline]
fn enable_access() {
    FPU_ACCESS_ENABLED[core_index()].store(true, Ordering::Release);
    clts();
}

#[inline]
fn access_enabled() -> bool {
    FPU_ACCESS_ENABLED[core_index()].load(Ordering::Acquire)
}

#[inline]
fn current_owner() -> *mut Tcb {
    FPU_OWNER[core_index()].load(Ordering::Acquire) as *mut Tcb
}

fn owner_core(thread: *const Tcb) -> Option<usize> {
    if thread.is_null() {
        return None;
    }
    let target = thread as usize;
    let mut core = 0;
    while core < MAX_NUM_NODES {
        if FPU_OWNER[core].load(Ordering::Acquire) == target {
            return Some(core);
        }
        core += 1;
    }
    None
}

unsafe fn switch_local_owner(new_owner: *mut Tcb) {
    let core = core_index();
    let old_owner = FPU_OWNER[core].load(Ordering::Acquire) as *mut Tcb;
    enable_access();
    if !old_owner.is_null() {
        unsafe { save_fpu_state(old_owner) };
    }
    if new_owner.is_null() {
        disable_access();
    } else {
        unsafe { load_fpu_state(new_owner) };
        unsafe { tcb::set_fpu_context_enabled(new_owner, access_enabled()) };
    }
    FPU_OWNER[core].store(new_owner as usize, Ordering::Release);
}

pub fn lazy_restore(thread: *mut Tcb) {
    if thread.is_null() {
        return;
    }
    if tcb::fpu_disabled_snapshot(thread) {
        disable_access();
        unsafe { tcb::set_fpu_context_enabled(thread, false) };
        return;
    }

    if current_owner() == thread {
        enable_access();
        unsafe { tcb::set_fpu_context_enabled(thread, access_enabled()) };
    } else {
        unsafe { switch_local_owner(thread) };
    }
}

pub fn handle_device_not_available(thread: *mut Tcb) -> bool {
    if thread.is_null() || tcb::fpu_disabled_snapshot(thread) {
        return false;
    }
    lazy_restore(thread);
    true
}

pub fn release(thread: *mut Tcb) {
    let Some(core) = owner_core(thread) else {
        return;
    };
    if core == core_index() {
        release_on_current_core(thread);
    } else {
        crate::kernel::smp::remote_fpu_owner_release(core, thread);
    }
}

pub fn release_on_current_core(thread: *mut Tcb) {
    if thread.is_null() || current_owner() != thread {
        return;
    }
    unsafe { switch_local_owner(null_mut()) };
}
