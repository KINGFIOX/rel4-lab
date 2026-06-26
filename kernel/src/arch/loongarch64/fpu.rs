//! LoongArch64 FPU hooks.
//!
//! This follows the same seL4-style lazy owner model as the RISC-V backend:
//! each core tracks the TCB whose scalar FPU state is currently live, saves it
//! on owner changes, and honors the TCB `fpuDisabled` flag. LSX/LASX remain
//! disabled by policy and are not saved in the user context.

use core::arch::asm;
use core::ptr::null_mut;
use core::sync::atomic::{AtomicBool, AtomicUsize, Ordering};

use crate::abi::constants::MAX_NUM_NODES;
use crate::arch::loongarch64::csr;
use crate::object::tcb::{self, Tcb};

const EUEN_FPE: usize = 1 << 0;
const EUEN_SXE: usize = 1 << 1;
const EUEN_ASXE: usize = 1 << 2;
const EUEN_FPU_STATE_MASK: usize = EUEN_FPE | EUEN_SXE | EUEN_ASXE;
const EUEN_VECTOR_STATE_MASK: usize = EUEN_SXE | EUEN_ASXE;
pub(crate) const EUEN_FPU_STATE_CLEAR_MASK: i64 = !(EUEN_FPU_STATE_MASK as i64);

static FPU_OWNER: [AtomicUsize; MAX_NUM_NODES] = [const { AtomicUsize::new(0) }; MAX_NUM_NODES];
static FPU_ACCESS_ENABLED: [AtomicBool; MAX_NUM_NODES] =
    [const { AtomicBool::new(false) }; MAX_NUM_NODES];

#[inline]
fn core_index() -> usize {
    crate::kernel::smp::current_core_id().min(MAX_NUM_NODES.saturating_sub(1))
}

#[inline]
fn clear_fpu_enable() {
    let euen = csr::euen();
    csr::set_euen(euen & !EUEN_FPU_STATE_MASK);
    csr::dbar();
}

#[inline]
fn set_scalar_fpu_enable() {
    let euen = csr::euen();
    csr::set_euen((euen | EUEN_FPE) & !EUEN_VECTOR_STATE_MASK);
    csr::dbar();
}

#[inline]
fn read_fcsr() -> u32 {
    let value: usize;
    unsafe {
        asm!(
            "movfcsr2gr {value}, $fcsr0",
            value = out(reg) value,
            options(nostack, nomem)
        );
    }
    value as u32
}

#[inline]
fn write_fcsr(value: u32) {
    unsafe {
        asm!(
            "movgr2fcsr $fcsr0, {value}",
            value = in(reg) value as usize,
            options(nostack, nomem)
        );
    }
}

#[inline]
fn read_fcc() -> u64 {
    let fcc0: usize;
    let fcc1: usize;
    let fcc2: usize;
    let fcc3: usize;
    let fcc4: usize;
    let fcc5: usize;
    let fcc6: usize;
    let fcc7: usize;
    unsafe {
        asm!(
            "movcf2gr {fcc0}, $fcc0",
            "movcf2gr {fcc1}, $fcc1",
            "movcf2gr {fcc2}, $fcc2",
            "movcf2gr {fcc3}, $fcc3",
            "movcf2gr {fcc4}, $fcc4",
            "movcf2gr {fcc5}, $fcc5",
            "movcf2gr {fcc6}, $fcc6",
            "movcf2gr {fcc7}, $fcc7",
            fcc0 = out(reg) fcc0,
            fcc1 = out(reg) fcc1,
            fcc2 = out(reg) fcc2,
            fcc3 = out(reg) fcc3,
            fcc4 = out(reg) fcc4,
            fcc5 = out(reg) fcc5,
            fcc6 = out(reg) fcc6,
            fcc7 = out(reg) fcc7,
            options(nostack, nomem)
        );
    }
    ((fcc0 & 1) as u64)
        | (((fcc1 & 1) as u64) << 1)
        | (((fcc2 & 1) as u64) << 2)
        | (((fcc3 & 1) as u64) << 3)
        | (((fcc4 & 1) as u64) << 4)
        | (((fcc5 & 1) as u64) << 5)
        | (((fcc6 & 1) as u64) << 6)
        | (((fcc7 & 1) as u64) << 7)
}

#[inline]
fn write_fcc(fcc: u64) {
    let fcc0 = (fcc & 1) as usize;
    let fcc1 = ((fcc >> 1) & 1) as usize;
    let fcc2 = ((fcc >> 2) & 1) as usize;
    let fcc3 = ((fcc >> 3) & 1) as usize;
    let fcc4 = ((fcc >> 4) & 1) as usize;
    let fcc5 = ((fcc >> 5) & 1) as usize;
    let fcc6 = ((fcc >> 6) & 1) as usize;
    let fcc7 = ((fcc >> 7) & 1) as usize;
    unsafe {
        asm!(
            "movgr2cf $fcc0, {fcc0}",
            "movgr2cf $fcc1, {fcc1}",
            "movgr2cf $fcc2, {fcc2}",
            "movgr2cf $fcc3, {fcc3}",
            "movgr2cf $fcc4, {fcc4}",
            "movgr2cf $fcc5, {fcc5}",
            "movgr2cf $fcc6, {fcc6}",
            "movgr2cf $fcc7, {fcc7}",
            fcc0 = in(reg) fcc0,
            fcc1 = in(reg) fcc1,
            fcc2 = in(reg) fcc2,
            fcc3 = in(reg) fcc3,
            fcc4 = in(reg) fcc4,
            fcc5 = in(reg) fcc5,
            fcc6 = in(reg) fcc6,
            fcc7 = in(reg) fcc7,
            options(nostack, nomem)
        );
    }
}

pub fn init_current_core() {
    let core = core_index();
    FPU_OWNER[core].store(0, Ordering::Release);
    set_scalar_fpu_enable();
    write_fcsr(0);
    write_fcc(0);
    clear_fpu_enable();
}

pub fn clear_supervisor_access() {
    clear_fpu_enable();
}

#[inline]
pub fn disable_access() {
    FPU_ACCESS_ENABLED[core_index()].store(false, Ordering::Release);
    clear_fpu_enable();
}

#[inline]
fn enable_access() {
    FPU_ACCESS_ENABLED[core_index()].store(true, Ordering::Release);
    set_scalar_fpu_enable();
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

unsafe fn save_fpu_state(thread: *mut Tcb) {
    if thread.is_null() {
        return;
    }
    let dest = unsafe { &mut (*thread).context.fpu };
    let regs = dest.regs.as_mut_ptr();
    set_scalar_fpu_enable();
    unsafe {
        asm!(
            "fst.d $f0,   {regs}, 0*8",
            "fst.d $f1,   {regs}, 1*8",
            "fst.d $f2,   {regs}, 2*8",
            "fst.d $f3,   {regs}, 3*8",
            "fst.d $f4,   {regs}, 4*8",
            "fst.d $f5,   {regs}, 5*8",
            "fst.d $f6,   {regs}, 6*8",
            "fst.d $f7,   {regs}, 7*8",
            "fst.d $f8,   {regs}, 8*8",
            "fst.d $f9,   {regs}, 9*8",
            "fst.d $f10, {regs}, 10*8",
            "fst.d $f11, {regs}, 11*8",
            "fst.d $f12, {regs}, 12*8",
            "fst.d $f13, {regs}, 13*8",
            "fst.d $f14, {regs}, 14*8",
            "fst.d $f15, {regs}, 15*8",
            "fst.d $f16, {regs}, 16*8",
            "fst.d $f17, {regs}, 17*8",
            "fst.d $f18, {regs}, 18*8",
            "fst.d $f19, {regs}, 19*8",
            "fst.d $f20, {regs}, 20*8",
            "fst.d $f21, {regs}, 21*8",
            "fst.d $f22, {regs}, 22*8",
            "fst.d $f23, {regs}, 23*8",
            "fst.d $f24, {regs}, 24*8",
            "fst.d $f25, {regs}, 25*8",
            "fst.d $f26, {regs}, 26*8",
            "fst.d $f27, {regs}, 27*8",
            "fst.d $f28, {regs}, 28*8",
            "fst.d $f29, {regs}, 29*8",
            "fst.d $f30, {regs}, 30*8",
            "fst.d $f31, {regs}, 31*8",
            regs = in(reg) regs,
            options(nostack),
        );
    }
    dest.fcsr = read_fcsr();
    dest.fcc = read_fcc();
}

unsafe fn load_fpu_state(thread: *const Tcb) {
    if thread.is_null() {
        return;
    }
    let src = unsafe { &(*thread).context.fpu };
    let regs = src.regs.as_ptr();
    set_scalar_fpu_enable();
    unsafe {
        asm!(
            "fld.d $f0,   {regs}, 0*8",
            "fld.d $f1,   {regs}, 1*8",
            "fld.d $f2,   {regs}, 2*8",
            "fld.d $f3,   {regs}, 3*8",
            "fld.d $f4,   {regs}, 4*8",
            "fld.d $f5,   {regs}, 5*8",
            "fld.d $f6,   {regs}, 6*8",
            "fld.d $f7,   {regs}, 7*8",
            "fld.d $f8,   {regs}, 8*8",
            "fld.d $f9,   {regs}, 9*8",
            "fld.d $f10, {regs}, 10*8",
            "fld.d $f11, {regs}, 11*8",
            "fld.d $f12, {regs}, 12*8",
            "fld.d $f13, {regs}, 13*8",
            "fld.d $f14, {regs}, 14*8",
            "fld.d $f15, {regs}, 15*8",
            "fld.d $f16, {regs}, 16*8",
            "fld.d $f17, {regs}, 17*8",
            "fld.d $f18, {regs}, 18*8",
            "fld.d $f19, {regs}, 19*8",
            "fld.d $f20, {regs}, 20*8",
            "fld.d $f21, {regs}, 21*8",
            "fld.d $f22, {regs}, 22*8",
            "fld.d $f23, {regs}, 23*8",
            "fld.d $f24, {regs}, 24*8",
            "fld.d $f25, {regs}, 25*8",
            "fld.d $f26, {regs}, 26*8",
            "fld.d $f27, {regs}, 27*8",
            "fld.d $f28, {regs}, 28*8",
            "fld.d $f29, {regs}, 29*8",
            "fld.d $f30, {regs}, 30*8",
            "fld.d $f31, {regs}, 31*8",
            regs = in(reg) regs,
            options(nostack),
        );
    }
    write_fcsr(src.fcsr);
    write_fcc(src.fcc);
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
