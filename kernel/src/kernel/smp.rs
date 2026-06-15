//! SMP substrate shared by boot, trap handling, and scheduling.
//!
//! User threads may run on multiple harts. This module keeps the per-hart
//! state that the trap path and scheduler need while a temporary big kernel
//! lock still serialises most shared kernel data-structure mutation.

#![allow(dead_code)]

use core::cell::UnsafeCell;
use core::hint;
use core::mem::size_of;
use core::ptr::null_mut;
use core::sync::atomic::{AtomicBool, AtomicU64, AtomicUsize, Ordering};

use crate::abi::constants::MAX_NUM_NODES;
use crate::api::thread::Thread;
use crate::object::tcb::Tcb;

pub const MAX_BOOT_HARTS: usize = 8;
pub const KERNEL_STACK_BYTES: usize = 64 * 1024;

unsafe extern "C" {
    static __stack_top: u8;
}

/// Per-hart trap scratch record addressed through `sscratch`.
///
/// `trap.S` relies on this exact layout. Keep the field order in sync with the
/// `TRAP_SCRATCH_*` offsets in that file.
#[repr(C)]
pub struct TrapScratch {
    kernel_stack_top: usize,
    user_context: usize,
    saved_user_sp: usize,
    saved_user_t1: usize,
    saved_user_t2: usize,
    core_id: usize,
    hart_id: usize,
}

const _: () = {
    assert!(size_of::<TrapScratch>() == 7 * size_of::<usize>());
    assert!(core::mem::offset_of!(TrapScratch, kernel_stack_top) == 0);
    assert!(core::mem::offset_of!(TrapScratch, user_context) == 1 * size_of::<usize>());
    assert!(core::mem::offset_of!(TrapScratch, saved_user_sp) == 2 * size_of::<usize>());
    assert!(core::mem::offset_of!(TrapScratch, saved_user_t1) == 3 * size_of::<usize>());
    assert!(core::mem::offset_of!(TrapScratch, saved_user_t2) == 4 * size_of::<usize>());
    assert!(core::mem::offset_of!(TrapScratch, core_id) == 5 * size_of::<usize>());
    assert!(core::mem::offset_of!(TrapScratch, hart_id) == 6 * size_of::<usize>());
};

impl TrapScratch {
    const fn new() -> Self {
        Self {
            kernel_stack_top: 0,
            user_context: 0,
            saved_user_sp: 0,
            saved_user_t1: 0,
            saved_user_t2: 0,
            core_id: usize::MAX,
            hart_id: usize::MAX,
        }
    }
}

struct TrapScratchCell(UnsafeCell<TrapScratch>);

unsafe impl Sync for TrapScratchCell {}

impl TrapScratchCell {
    const fn new() -> Self {
        Self(UnsafeCell::new(TrapScratch::new()))
    }

    fn get(&self) -> *mut TrapScratch {
        self.0.get()
    }
}

struct ThreadCell(UnsafeCell<Thread>);

unsafe impl Sync for ThreadCell {}

impl ThreadCell {
    const fn new() -> Self {
        Self(UnsafeCell::new(Thread::null()))
    }

    fn with_mut<R>(&self, op: impl FnOnce(&mut Thread) -> R) -> R {
        debug_assert_kernel_lock_held();
        let thread = unsafe { &mut *self.0.get() };
        op(thread)
    }
}

struct HartState {
    hart_id: AtomicUsize,
    core_id: AtomicUsize,
    online: AtomicBool,
    trap_scratch: TrapScratchCell,
    current_tcb: AtomicUsize,
    thread: ThreadCell,
    next_timer_deadline: AtomicU64,
}

impl HartState {
    const fn new() -> Self {
        Self {
            hart_id: AtomicUsize::new(usize::MAX),
            core_id: AtomicUsize::new(usize::MAX),
            online: AtomicBool::new(false),
            trap_scratch: TrapScratchCell::new(),
            current_tcb: AtomicUsize::new(0),
            thread: ThreadCell::new(),
            next_timer_deadline: AtomicU64::new(0),
        }
    }
}

static HARTS: [HartState; MAX_BOOT_HARTS] = [const { HartState::new() }; MAX_BOOT_HARTS];
static KERNEL_LOCK: SpinLock = SpinLock::new();
static KERNEL_LOCK_OWNER: AtomicUsize = AtomicUsize::new(NO_KERNEL_LOCK_OWNER);
static KERNEL_SATP: AtomicU64 = AtomicU64::new(0);
static REMOTE_STALL_PENDING_MASK: AtomicUsize = AtomicUsize::new(0);
static REMOTE_STALL_DONE_MASK: AtomicUsize = AtomicUsize::new(0);
static REMOTE_STALL_TARGET_VALUE: AtomicUsize = AtomicUsize::new(0);
static REMOTE_STALL_OP: AtomicUsize = AtomicUsize::new(REMOTE_OP_STALL_TCB);

const NO_KERNEL_LOCK_OWNER: usize = usize::MAX;
const REMOTE_OP_STALL_TCB: usize = 1;
const REMOTE_OP_RELEASE_FPU_OWNER: usize = 2;
const REMOTE_OP_FLUSH_VMA_ALL: usize = 3;
const REMOTE_OP_FLUSH_VMA_ASID: usize = 4;
pub const SECONDARY_BOOT_WAIT_MAGIC: usize = 0x534d_5057_4149_5421;
pub const SECONDARY_BOOT_READY_MAGIC: usize = 0x534d_5052_4541_4459;

#[unsafe(link_section = ".boot.data")]
pub static SECONDARY_BOOT_READY: AtomicUsize = AtomicUsize::new(SECONDARY_BOOT_WAIT_MAGIC);

pub struct SpinLock {
    locked: AtomicBool,
}

pub struct SpinLockGuard<'a> {
    lock: &'a SpinLock,
    irq_was_enabled: bool,
    remote_stalled_current: bool,
}

#[derive(Copy, Clone, Debug, Eq, PartialEq)]
pub(crate) enum RemoteCoreOpResult {
    None,
    Serviced,
    StalledCurrent,
}

impl SpinLock {
    pub const fn new() -> Self {
        Self {
            locked: AtomicBool::new(false),
        }
    }

    pub fn lock(&self) -> SpinLockGuard<'_> {
        let irq_was_enabled = crate::arch::current::irq::local_irq_save();
        let mut remote_stalled_current = false;
        while self
            .locked
            .compare_exchange(false, true, Ordering::Acquire, Ordering::Relaxed)
            .is_err()
        {
            remote_stalled_current |=
                service_pending_remote_core_op() == RemoteCoreOpResult::StalledCurrent;
            hint::spin_loop();
        }
        SpinLockGuard {
            lock: self,
            irq_was_enabled,
            remote_stalled_current,
        }
    }
}

impl Drop for SpinLockGuard<'_> {
    fn drop(&mut self) {
        self.lock.locked.store(false, Ordering::Release);
        crate::arch::current::irq::local_irq_restore(self.irq_was_enabled);
    }
}

pub struct KernelLockGuard(SpinLockGuard<'static>);

impl KernelLockGuard {
    pub fn lock() -> Self {
        let guard = KERNEL_LOCK.lock();
        KERNEL_LOCK_OWNER.store(current_core_id(), Ordering::Release);
        Self(guard)
    }

    pub fn remote_stalled_current(&self) -> bool {
        self.0.remote_stalled_current
    }

    pub fn defer_unlock_for_user_restore(self) {
        debug_assert_kernel_lock_held();
        core::mem::forget(self);
    }
}

impl Drop for KernelLockGuard {
    fn drop(&mut self) {
        debug_assert_kernel_lock_held();
        KERNEL_LOCK_OWNER.store(NO_KERNEL_LOCK_OWNER, Ordering::Release);
    }
}

#[inline]
pub fn kernel_lock_is_held_by_current_core() -> bool {
    KERNEL_LOCK_OWNER.load(Ordering::Acquire) == current_core_id()
}

#[inline]
fn kernel_state_is_serialized() -> bool {
    kernel_lock_is_held_by_current_core()
        || SECONDARY_BOOT_READY.load(Ordering::Acquire) != SECONDARY_BOOT_READY_MAGIC
}

#[track_caller]
#[inline]
pub fn debug_assert_kernel_lock_held() {
    debug_assert!(
        kernel_state_is_serialized(),
        "kernel object mutation requires the seL4-style big kernel lock"
    );
}

#[derive(Copy, Clone, Debug, Default)]
pub struct BklObjectGuard;

impl BklObjectGuard {
    #[inline]
    pub fn new() -> Self {
        debug_assert_kernel_lock_held();
        Self
    }
}

pub struct BklCell<T> {
    value: UnsafeCell<T>,
}

unsafe impl<T: Send> Sync for BklCell<T> {}

impl<T> BklCell<T> {
    pub const fn new(value: T) -> Self {
        Self {
            value: UnsafeCell::new(value),
        }
    }

    #[inline]
    pub fn with_ref<R>(&self, op: impl FnOnce(&T) -> R) -> R {
        debug_assert_kernel_lock_held();
        unsafe { op(&*self.value.get()) }
    }

    #[inline]
    pub fn with_mut<R>(&self, op: impl FnOnce(&mut T) -> R) -> R {
        debug_assert_kernel_lock_held();
        unsafe { op(&mut *self.value.get()) }
    }
}

#[unsafe(no_mangle)]
pub extern "C" fn kernel_unlock_for_user_restore() {
    debug_assert_kernel_lock_held();
    KERNEL_LOCK_OWNER.store(NO_KERNEL_LOCK_OWNER, Ordering::Release);
    KERNEL_LOCK.locked.store(false, Ordering::Release);
}

#[inline]
fn kernel_stack_top_for_core(core_id: usize) -> usize {
    let stack_top = unsafe { &__stack_top as *const u8 as usize };
    stack_top - core_id * KERNEL_STACK_BYTES
}

#[inline]
pub fn current_core_id() -> usize {
    let scratch = crate::arch::current::csr::sscratch() as *const TrapScratch;
    if scratch.is_null() {
        return 0;
    }
    let core_id = unsafe { (*scratch).core_id };
    if core_id < MAX_NUM_NODES { core_id } else { 0 }
}

#[inline]
fn current_hart() -> &'static HartState {
    &HARTS[current_core_id()]
}

pub fn init_current_hart(hart_id: usize, core_id: usize) {
    assert!(core_id < MAX_BOOT_HARTS, "core_id exceeds hart-state table");
    assert!(core_id < MAX_NUM_NODES, "core_id exceeds configured nodes");

    let hart = &HARTS[core_id];
    hart.hart_id.store(hart_id, Ordering::Release);
    hart.core_id.store(core_id, Ordering::Release);

    unsafe {
        let scratch = &mut *hart.trap_scratch.get();
        scratch.kernel_stack_top = kernel_stack_top_for_core(core_id);
        scratch.user_context = 0;
        scratch.saved_user_sp = 0;
        scratch.saved_user_t1 = 0;
        scratch.saved_user_t2 = 0;
        scratch.core_id = core_id;
        scratch.hart_id = hart_id;
        crate::arch::current::csr::set_sscratch(scratch as *mut TrapScratch as usize);
    }

    hart.online.store(true, Ordering::Release);
}

pub fn release_secondary_harts() {
    SECONDARY_BOOT_READY.store(SECONDARY_BOOT_READY_MAGIC, Ordering::Release);
}

pub fn publish_kernel_satp(satp: u64) {
    KERNEL_SATP.store(satp, Ordering::Release);
}

pub fn kernel_satp() -> Option<u64> {
    match KERNEL_SATP.load(Ordering::Acquire) {
        0 => None,
        satp => Some(satp),
    }
}

pub fn wake_core(core_id: usize) {
    if core_id >= MAX_NUM_NODES || core_id == current_core_id() {
        return;
    }
    let Some(hart_id) = remote_online_hart_id(core_id) else {
        return;
    };
    assert_remote_ipi_supported("wake_core");
    let ret = crate::arch::current::sbi::send_ipi(1, hart_id);
    assert!(
        ret.error == 0,
        "SBI send_ipi failed for core {core_id} hart {hart_id}: error={}",
        ret.error
    );
}

pub fn current_core_of_tcb(tcb: *const Tcb) -> Option<usize> {
    if tcb.is_null() {
        return None;
    }
    let target = tcb as usize;
    let mut core = 0;
    while core < MAX_NUM_NODES && core < MAX_BOOT_HARTS {
        let hart = &HARTS[core];
        if hart.online.load(Ordering::Acquire) && hart.current_tcb.load(Ordering::Acquire) == target
        {
            return Some(core);
        }
        core += 1;
    }
    None
}

pub fn wake_current_core_of_tcb(tcb: *const Tcb) {
    if let Some(core) = current_core_of_tcb(tcb) {
        wake_core(core);
    }
}

pub fn remote_tcb_stall(tcb: *const Tcb) {
    debug_assert_kernel_lock_held();
    let Some(core) = current_core_of_tcb(tcb) else {
        return;
    };
    if core == current_core_id() {
        return;
    }
    remote_core_op(core, REMOTE_OP_STALL_TCB, tcb as usize);
}

pub fn remote_fpu_owner_release(core: usize, tcb: *const Tcb) {
    debug_assert_kernel_lock_held();
    if tcb.is_null() || core >= MAX_NUM_NODES || core == current_core_id() {
        return;
    }
    remote_core_op(core, REMOTE_OP_RELEASE_FPU_OWNER, tcb as usize);
}

fn remote_core_op(core: usize, op: usize, target_value: usize) {
    let Some(bit) = core_bit(core) else {
        return;
    };
    if remote_online_hart_id(core).is_none() {
        return;
    }
    assert_remote_ipi_supported("remote_core_op");

    REMOTE_STALL_TARGET_VALUE.store(target_value, Ordering::Release);
    REMOTE_STALL_OP.store(op, Ordering::Release);
    REMOTE_STALL_DONE_MASK.store(0, Ordering::Release);
    REMOTE_STALL_PENDING_MASK.store(bit, Ordering::Release);
    wake_core(core);

    while REMOTE_STALL_DONE_MASK.load(Ordering::Acquire) & bit == 0 {
        hint::spin_loop();
    }

    REMOTE_STALL_PENDING_MASK.store(0, Ordering::Release);
    REMOTE_STALL_TARGET_VALUE.store(0, Ordering::Release);
    REMOTE_STALL_OP.store(REMOTE_OP_STALL_TCB, Ordering::Release);
}

#[inline]
fn core_bit(core: usize) -> Option<usize> {
    if core < usize::BITS as usize {
        Some(1usize << core)
    } else {
        None
    }
}

#[inline]
fn remote_online_hart_id(core: usize) -> Option<usize> {
    if core >= MAX_NUM_NODES || core >= MAX_BOOT_HARTS || core == current_core_id() {
        return None;
    }
    let hart = &HARTS[core];
    if !hart.online.load(Ordering::Acquire) {
        return None;
    }
    let hart_id = hart.hart_id.load(Ordering::Acquire);
    (hart_id != usize::MAX).then_some(hart_id)
}

fn assert_remote_ipi_supported(context: &str) {
    assert!(
        crate::arch::current::sbi::SUPPORTS_REMOTE_IPI,
        "{context}: remote IPI requested before this architecture has an IPI backend"
    );
}

fn assert_remote_tlb_flush_supported(context: &str) {
    assert!(
        crate::arch::current::sbi::SUPPORTS_REMOTE_TLB_FLUSH,
        "{context}: remote TLB flush requested before this architecture has an RFENCE backend"
    );
}

/// Service a pending remote operation for the current core.
///
/// This is used both while spinning for the BKL and after a LoongArch IPI trap
/// has acquired it; in the latter case a remote TCB stall must avoid resuming
/// the just-interrupted user context.
pub(crate) fn service_pending_remote_core_op() -> RemoteCoreOpResult {
    let Some(bit) = core_bit(current_core_id()) else {
        return RemoteCoreOpResult::None;
    };
    if REMOTE_STALL_PENDING_MASK.load(Ordering::Acquire) & bit == 0 {
        return RemoteCoreOpResult::None;
    }
    if REMOTE_STALL_DONE_MASK.load(Ordering::Acquire) & bit != 0 {
        return RemoteCoreOpResult::None;
    }

    let target = REMOTE_STALL_TARGET_VALUE.load(Ordering::Acquire);
    let op = REMOTE_STALL_OP.load(Ordering::Acquire);
    // seL4 keeps ordinary remote TCB stall separate from the remote FPU
    // owner switch; the latter saves and clears the FPU owner without
    // descheduling the target TCB.
    match op {
        REMOTE_OP_RELEASE_FPU_OWNER => {
            if target != 0 {
                crate::arch::current::fpu::release_on_current_core(target as *mut Tcb);
            }
            complete_remote_core_op(bit);
            return RemoteCoreOpResult::Serviced;
        }
        REMOTE_OP_FLUSH_VMA_ALL => {
            crate::arch::current::csr::sfence_vma_all();
            complete_remote_core_op(bit);
            return RemoteCoreOpResult::Serviced;
        }
        REMOTE_OP_FLUSH_VMA_ASID => {
            crate::arch::current::csr::sfence_vma_asid(target);
            complete_remote_core_op(bit);
            return RemoteCoreOpResult::Serviced;
        }
        _ => {}
    }
    let hart = current_hart();
    let stalled_current = target != 0 && hart.current_tcb.load(Ordering::Acquire) == target;
    if stalled_current {
        hart.current_tcb
            .store(null_mut::<Tcb>() as usize, Ordering::Release);
        unsafe {
            (*hart.trap_scratch.get()).user_context = 0;
        }
    }
    complete_remote_core_op(bit);
    if stalled_current {
        RemoteCoreOpResult::StalledCurrent
    } else {
        RemoteCoreOpResult::Serviced
    }
}

fn complete_remote_core_op(bit: usize) {
    #[cfg(target_arch = "loongarch64")]
    crate::arch::current::sbi::ack_ipi();
    REMOTE_STALL_DONE_MASK.fetch_or(bit, Ordering::AcqRel);
}

pub fn remote_sfence_vma_all() {
    let mut core = 0;
    while core < MAX_NUM_NODES {
        if let Some(hart_id) = remote_online_hart_id(core) {
            remote_sfence_vma_core(core, hart_id);
        }
        core += 1;
    }
}

pub fn remote_sfence_vma_asid_all(asid: usize) {
    let mut core = 0;
    while core < MAX_NUM_NODES {
        if let Some(hart_id) = remote_online_hart_id(core) {
            remote_sfence_vma_asid_core(core, hart_id, asid);
        }
        core += 1;
    }
}

#[cfg(target_arch = "riscv64")]
fn remote_sfence_vma_core(core: usize, hart_id: usize) {
    assert_remote_tlb_flush_supported("remote_sfence_vma_all");
    let ret = crate::arch::current::sbi::remote_sfence_vma(1, hart_id, 0, 0);
    assert!(
        ret.error == 0,
        "SBI remote_sfence_vma failed for core {core} hart {hart_id}: error={}",
        ret.error
    );
}

#[cfg(target_arch = "riscv64")]
fn remote_sfence_vma_asid_core(core: usize, hart_id: usize, asid: usize) {
    assert_remote_tlb_flush_supported("remote_sfence_vma_asid_all");
    let ret = crate::arch::current::sbi::remote_sfence_vma_asid(1, hart_id, 0, 0, asid);
    assert!(
        ret.error == 0,
        "SBI remote_sfence_vma_asid failed for core {core} hart {hart_id}: error={}",
        ret.error
    );
}

#[cfg(target_arch = "loongarch64")]
fn remote_sfence_vma_core(core: usize, _hart_id: usize) {
    remote_core_op(core, REMOTE_OP_FLUSH_VMA_ALL, 0);
}

#[cfg(target_arch = "loongarch64")]
fn remote_sfence_vma_asid_core(core: usize, _hart_id: usize, asid: usize) {
    remote_core_op(core, REMOTE_OP_FLUSH_VMA_ASID, asid);
}

pub fn sfence_vma_all_harts() {
    crate::arch::current::csr::sfence_vma_all();
    remote_sfence_vma_all();
}

pub fn sfence_vma_asid_all_harts(asid: usize) {
    crate::arch::current::csr::sfence_vma_asid(asid);
    remote_sfence_vma_asid_all(asid);
}

#[inline]
pub fn current_tcb() -> *mut Tcb {
    current_hart().current_tcb.load(Ordering::Acquire) as *mut Tcb
}

#[inline]
pub fn set_current_tcb(tcb: *mut Tcb) -> *mut Tcb {
    debug_assert_kernel_lock_held();
    current_hart()
        .current_tcb
        .swap(tcb as usize, Ordering::AcqRel) as *mut Tcb
}

pub unsafe fn set_current_thread(thread: Thread) {
    current_hart().thread.with_mut(|current| *current = thread);
}

pub unsafe fn with_current_thread<R>(op: impl FnOnce(&mut Thread) -> R) -> R {
    current_hart().thread.with_mut(op)
}

#[inline]
pub fn next_timer_deadline() -> u64 {
    current_hart().next_timer_deadline.load(Ordering::Acquire)
}

#[inline]
pub fn set_next_timer_deadline(deadline: u64) {
    current_hart()
        .next_timer_deadline
        .store(deadline, Ordering::Release);
}

pub fn clear_current_state() {
    debug_assert_kernel_lock_held();
    let hart = current_hart();
    hart.current_tcb
        .store(null_mut::<Tcb>() as usize, Ordering::Release);
    hart.thread.with_mut(|thread| *thread = Thread::null());
}
