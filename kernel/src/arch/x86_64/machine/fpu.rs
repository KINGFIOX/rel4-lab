//! x86_64 FPU ownership and lazy context switching.
//!
//! Hardware state is saved with `fxsave` / restored with `fxrstor`. `CR0.TS`
//! is set when the current user thread must not use the FPU; kernel entry
//! always clears TS so compiler-generated SSE in the kernel cannot #NM.

use core::arch::asm;
use core::sync::atomic::{AtomicBool, AtomicUsize, Ordering};

use crate::abi::constants::MAX_NUM_NODES;
use crate::ktypes::objref::{ObjRef, OptObjRefExt};
use crate::object::tcb::TcbRef;

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
    // SAFETY: clearing `CR0.TS` only re-enables FPU access for this core.
    unsafe {
        asm!("clts", options(nostack, nomem, preserves_flags));
    }
}

#[inline]
fn set_ts() {
    // SAFETY: setting `CR0.TS` only makes the next FPU use trap.
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

fn save_fpu_state(thread: TcbRef) {
    thread.with_context_mut(|context| {
        let dest = context.fpu.fxsave.as_mut_ptr();
        clts();
        // SAFETY: `dest` is the thread's own 16-byte-aligned FXSAVE area,
        // borrowed for the duration of the instruction.
        unsafe {
            asm!(
                "fxsave64 [{0}]",
                in(reg) dest,
                options(nostack, preserves_flags),
            );
        }
    });
}

fn load_fpu_state(thread: TcbRef) {
    thread.with_context(|context| {
        let src = context.fpu.fxsave.as_ptr();
        clts();
        // SAFETY: `src` is the thread's own FXSAVE area, holding state this
        // kernel previously saved there.
        unsafe {
            asm!(
                "fxrstor64 [{0}]",
                in(reg) src,
                options(nostack, preserves_flags),
            );
        }
    });
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
fn current_owner() -> Option<TcbRef> {
    owner_of_core(core_index())
}

/// The thread whose FPU state is loaded on `core`, if any.
#[inline]
fn owner_of_core(core: usize) -> Option<TcbRef> {
    // SAFETY: the word only ever holds an address stored from a live `TcbRef`
    // by `switch_local_owner`.
    unsafe { ObjRef::from_kva(FPU_OWNER[core].load(Ordering::Acquire) as u64) }
}

/// Which core, if any, currently holds `thread`'s FPU state.
fn owner_core(thread: TcbRef) -> Option<usize> {
    (0..MAX_NUM_NODES).find(|&core| owner_of_core(core) == Some(thread))
}

fn switch_local_owner(new_owner: Option<TcbRef>) {
    let core = core_index();
    let old_owner = current_owner();
    enable_access();
    if let Some(old_owner) = old_owner {
        save_fpu_state(old_owner);
    }
    match new_owner {
        None => disable_access(),
        Some(new_owner) => {
            load_fpu_state(new_owner);
            new_owner.set_fpu_context_enabled(access_enabled());
        }
    }
    FPU_OWNER[core].store(new_owner.kva_or_zero() as usize, Ordering::Release);
}

pub fn lazy_restore(thread: TcbRef) {
    if thread.fpu_disabled() {
        disable_access();
        thread.set_fpu_context_enabled(false);
        return;
    }

    if current_owner() == Some(thread) {
        enable_access();
        thread.set_fpu_context_enabled(access_enabled());
    } else {
        switch_local_owner(Some(thread));
    }
}

/// A `#NM` means the thread touched the FPU while `CR0.TS` was set. Load its
/// state and let it retry, unless FPU use is disabled for it.
pub fn handle_device_not_available(thread: Option<TcbRef>) -> bool {
    let Some(thread) = thread.filter(|thread| !thread.fpu_disabled()) else {
        return false;
    };
    lazy_restore(thread);
    true
}

pub fn release(thread: TcbRef) {
    let Some(core) = owner_core(thread) else {
        return;
    };
    if core == core_index() {
        release_on_current_core(thread);
    } else {
        crate::kernel::smp::remote_fpu_owner_release(core, thread);
    }
}

pub fn release_on_current_core(thread: TcbRef) {
    if current_owner() != Some(thread) {
        return;
    }
    switch_local_owner(None);
}
