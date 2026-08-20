//! RISC-V FPU ownership and lazy context switching.
//!
//! This follows upstream seL4's RISC-V model: each hart records the TCB whose
//! floating-point state is currently live in hardware, `TCBSetFlags` can disable
//! user FPU access for a TCB, and switching to a different FPU-using thread saves
//! the old owner and restores the new one.

use core::arch::asm;
use core::sync::atomic::{AtomicBool, AtomicUsize, Ordering};

use crate::abi::constants::MAX_NUM_NODES;
use crate::arch::riscv64::kernel::trap::{SSTATUS_FS_CLEAN, SSTATUS_FS_MASK};
use crate::ktypes::objref::{ObjRef, OptObjRefExt};
use crate::object::tcb::TcbRef;

static FPU_OWNER: [AtomicUsize; MAX_NUM_NODES] = [const { AtomicUsize::new(0) }; MAX_NUM_NODES];
static FPU_ACCESS_ENABLED: [AtomicBool; MAX_NUM_NODES] =
    [const { AtomicBool::new(false) }; MAX_NUM_NODES];

#[inline]
fn core_index() -> usize {
    crate::kernel::smp::current_core_id().min(MAX_NUM_NODES.saturating_sub(1))
}

#[inline]
fn set_fs_off() {
    // SAFETY: clearing `sstatus.FS` only disables FP access for this core.
    unsafe {
        asm!(
            "csrc sstatus, {mask}",
            mask = in(reg) SSTATUS_FS_MASK as usize,
            options(nostack, nomem)
        );
    }
}

#[inline]
fn set_fs_clean() {
    // SAFETY: setting `sstatus.FS` to Clean only enables FP access.
    unsafe {
        asm!(
            "csrs sstatus, {mask}",
            mask = in(reg) SSTATUS_FS_CLEAN as usize,
            options(nostack, nomem)
        );
    }
}

#[inline]
fn read_fcsr() -> u32 {
    let value: usize;
    // SAFETY: reading `fcsr` has no side effect beyond needing FP access, which
    // the callers enable first.
    unsafe { asm!("csrr {0}, fcsr", out(reg) value, options(nostack, nomem)) };
    value as u32
}

#[inline]
fn write_fcsr(value: u32) {
    // SAFETY: writing `fcsr` affects only FP rounding and exception state.
    unsafe { asm!("csrw fcsr, {0}", in(reg) value as usize, options(nostack, nomem)) };
}

pub fn init_current_core() {
    let core = core_index();
    FPU_OWNER[core].store(0, Ordering::Release);
    set_fs_clean();
    write_fcsr(0);
    disable_access();
}

pub fn clear_supervisor_access() {
    set_fs_off();
}

#[inline]
pub fn disable_access() {
    FPU_ACCESS_ENABLED[core_index()].store(false, Ordering::Release);
}

#[inline]
fn enable_access() {
    FPU_ACCESS_ENABLED[core_index()].store(true, Ordering::Release);
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

fn save_fpu_state(thread: TcbRef) {
    thread.with_context_mut(|context| {
        let dest = &mut context.fpu;
        let regs = dest.regs.as_mut_ptr();
        set_fs_clean();
        // SAFETY: `regs` is the thread's own 32-slot FP register save area,
        // borrowed for the duration of the instruction sequence.
        unsafe {
            asm!(
                "fsd f0,   0*8({regs})",
                "fsd f1,   1*8({regs})",
                "fsd f2,   2*8({regs})",
                "fsd f3,   3*8({regs})",
                "fsd f4,   4*8({regs})",
                "fsd f5,   5*8({regs})",
                "fsd f6,   6*8({regs})",
                "fsd f7,   7*8({regs})",
                "fsd f8,   8*8({regs})",
                "fsd f9,   9*8({regs})",
                "fsd f10, 10*8({regs})",
                "fsd f11, 11*8({regs})",
                "fsd f12, 12*8({regs})",
                "fsd f13, 13*8({regs})",
                "fsd f14, 14*8({regs})",
                "fsd f15, 15*8({regs})",
                "fsd f16, 16*8({regs})",
                "fsd f17, 17*8({regs})",
                "fsd f18, 18*8({regs})",
                "fsd f19, 19*8({regs})",
                "fsd f20, 20*8({regs})",
                "fsd f21, 21*8({regs})",
                "fsd f22, 22*8({regs})",
                "fsd f23, 23*8({regs})",
                "fsd f24, 24*8({regs})",
                "fsd f25, 25*8({regs})",
                "fsd f26, 26*8({regs})",
                "fsd f27, 27*8({regs})",
                "fsd f28, 28*8({regs})",
                "fsd f29, 29*8({regs})",
                "fsd f30, 30*8({regs})",
                "fsd f31, 31*8({regs})",
                regs = in(reg) regs,
                options(nostack),
            );
        }
        dest.fcsr = read_fcsr();
    });
}

fn load_fpu_state(thread: TcbRef) {
    thread.with_context(|context| {
        let src = &context.fpu;
        let regs = src.regs.as_ptr();
        set_fs_clean();
        // SAFETY: `regs` is the thread's own FP register save area, holding state
        // this kernel previously saved there.
        unsafe {
            asm!(
                "fld f0,   0*8({regs})",
                "fld f1,   1*8({regs})",
                "fld f2,   2*8({regs})",
                "fld f3,   3*8({regs})",
                "fld f4,   4*8({regs})",
                "fld f5,   5*8({regs})",
                "fld f6,   6*8({regs})",
                "fld f7,   7*8({regs})",
                "fld f8,   8*8({regs})",
                "fld f9,   9*8({regs})",
                "fld f10, 10*8({regs})",
                "fld f11, 11*8({regs})",
                "fld f12, 12*8({regs})",
                "fld f13, 13*8({regs})",
                "fld f14, 14*8({regs})",
                "fld f15, 15*8({regs})",
                "fld f16, 16*8({regs})",
                "fld f17, 17*8({regs})",
                "fld f18, 18*8({regs})",
                "fld f19, 19*8({regs})",
                "fld f20, 20*8({regs})",
                "fld f21, 21*8({regs})",
                "fld f22, 22*8({regs})",
                "fld f23, 23*8({regs})",
                "fld f24, 24*8({regs})",
                "fld f25, 25*8({regs})",
                "fld f26, 26*8({regs})",
                "fld f27, 27*8({regs})",
                "fld f28, 28*8({regs})",
                "fld f29, 29*8({regs})",
                "fld f30, 30*8({regs})",
                "fld f31, 31*8({regs})",
                regs = in(reg) regs,
                options(nostack),
            );
        }
        write_fcsr(src.fcsr);
    });
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
