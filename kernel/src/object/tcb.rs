//! Kernel-side TCB object.
//!
//! Lives in the 2 KiB (`seL4_TCBBits = 11`) region the user retypes from
//! an Untyped via `Untyped_Retype(seL4_TCBObject)`. Because that region
//! is always `2 KiB`-aligned and bigger than `size_of::<Tcb>()`, we can
//! safely treat the cap's pointer as a `*mut Tcb`.
//!
//! For now we do *not* yet have a scheduler — every TCB except the
//! rootserver stays in `ThreadState::Inactive`, and `TCB_Resume` flips
//! it to `Restart` but never actually re-enters the saved context. The
//! invocation handlers persist all configuration into the struct so
//! that a future iteration can implement context-switch without having
//! to revisit the parse / validate code.
//!
//! Layout-load: every field must fit comfortably inside the 2 KiB slab
//! the C kernel allocates, so the future C/Rust ABI swap stays valid.

#![allow(dead_code)]

use core::ptr::null_mut;
use core::sync::atomic::{AtomicPtr, Ordering};

use crate::arch::riscv64::trap::UserContext;
use crate::object::cap::Cap;

/// Pointer to the currently-scheduled TCB. Set at boot to the rootserver
/// TCB and updated whenever the scheduler swaps threads. Always read via
/// the accessors below to keep memory ordering consistent.
static CURRENT_TCB: AtomicPtr<Tcb> = AtomicPtr::new(null_mut());

#[inline]
pub fn current() -> *mut Tcb {
    CURRENT_TCB.load(Ordering::Acquire)
}

/// Replace `CURRENT_TCB`. Returns the previous pointer.
#[inline]
pub fn set_current(tcb: *mut Tcb) -> *mut Tcb {
    CURRENT_TCB.swap(tcb, Ordering::AcqRel)
}

pub const TCB_NAME_LEN: usize = 32;

/// Mirrors `_thread_state` in `kernel/include/object/structures.h`. The
/// numbering doesn't need to match upstream because we never expose this
/// over the ABI; we only use it for our own scheduler.
#[repr(u8)]
#[derive(Copy, Clone, PartialEq, Eq, Debug)]
pub enum ThreadState {
    Inactive = 0,
    Restart = 1,
    Running = 2,
    BlockedOnReceive = 3,
    BlockedOnSend = 4,
    BlockedOnReply = 5,
    BlockedOnNotification = 6,
    Idle = 7,
}

#[repr(C)]
pub struct Tcb {
    /// Saved user-mode register state. The trap path restores this on
    /// `sret` once a scheduler picks the TCB.
    pub context: UserContext,

    /// Scheduling.
    pub state: u8,
    pub priority: u8,
    pub mcp: u8,
    pub domain: u8,
    pub flags: u32,

    /// Cap roots. The full caps (not just pointers) so that the future
    /// `restore_user_context` path can re-lookup CSpace / VSpace without
    /// holding state outside the TCB.
    pub cspace_cap: Cap,
    pub vspace_cap: Cap,
    pub ipc_buffer_cap: Cap,

    /// User-mode VA at which the IPC buffer is mapped.
    pub ipc_buffer_uva: u64,
    /// Kernel-window VA reachable via the PSpace mapping of the IPC
    /// buffer frame — i.e. what `Thread.ipc_buffer_kva` would point at
    /// after `restore_user_context` swaps to this thread. Lazily
    /// resolved; 0 means "not yet set up".
    pub ipc_buffer_kva: u64,

    /// Fault handler endpoint, expressed as a CPtr in *this* TCB's
    /// CSpace (matches the C kernel's `tcb->tcbFaultHandler`).
    pub fault_ep_cptr: u64,

    /// Pointer (PSpace KVA) to the bound `Notification`, or 0.
    pub bound_notification: u64,

    /// Ready-queue links. Both are PSpace KVAs of other `Tcb`s, or 0.
    pub queue_next: u64,
    pub queue_prev: u64,

    /// Object the TCB is currently blocked on (Endpoint / Notification
    /// KVA), if any. Used to dequeue on cancel / destroy.
    pub waiting_on: u64,

    /// Debug name, populated by `seL4_DebugNameThread`. NUL-padded.
    pub name: [u8; TCB_NAME_LEN],
}

// Compile-time sanity: must fit inside the 2 KiB Untyped slab (= 2048
// bytes for SEL4_TCB_BITS = 11). Keep a generous margin so future
// fields (FPU context, MCS scheduler data) don't break the layout.
const _: () = {
    assert!(core::mem::size_of::<Tcb>() <= 1024);
};

impl Tcb {
    /// All-zero TCB constructor for static / BSS use (the rootserver TCB
    /// is created this way; user-allocated TCBs go through `init` after
    /// `Untyped_Retype` zeroes their slab).
    pub const fn zero() -> Self {
        Tcb {
            context: UserContext {
                regs: [0; 32],
                pc: 0,
                sstatus: 0,
                _reserved: 0,
            },
            state: 0,
            priority: 0,
            mcp: 0,
            domain: 0,
            flags: 0,
            cspace_cap: Cap::null(),
            vspace_cap: Cap::null(),
            ipc_buffer_cap: Cap::null(),
            ipc_buffer_uva: 0,
            ipc_buffer_kva: 0,
            fault_ep_cptr: 0,
            bound_notification: 0,
            queue_next: 0,
            queue_prev: 0,
            waiting_on: 0,
            name: [0; TCB_NAME_LEN],
        }
    }
}

/// Reborrow a Thread cap as a `*mut Tcb`. Returns `null_mut()` for a
/// null cap. Caller guarantees the underlying memory has not been
/// recycled (`finalize` clears caps before the Untyped is reset, so as
/// long as you go through the live cap you're safe).
#[inline]
pub fn from_cap(cap: Cap) -> *mut Tcb {
    let p = cap.thread_ptr();
    if p == 0 {
        null_mut()
    } else {
        p as *mut Tcb
    }
}

/// Initialise a freshly-retyped 2 KiB TCB slab.
///
/// `Untyped_Retype` already zeroed the memory; we only stamp the bits
/// where 0 isn't the right resting value (currently just sstatus so a
/// future `restore_user_context` returns to U-mode with interrupts on).
pub unsafe fn init(tcb_kva: u64) {
    let t = tcb_kva as *mut Tcb;
    // sstatus.SPIE = 1 -> sret re-enables interrupts in U-mode.
    // sstatus.SPP  = 0 -> sret enters U-mode (already 0).
    unsafe {
        (*t).state = ThreadState::Inactive as u8;
        (*t).context.sstatus = 1 << 5;
    }
}

/// Per-invocation primitives. They never block / never schedule — they
/// just update the data the future scheduler will key off.
pub unsafe fn suspend(tcb: *mut Tcb) {
    if tcb.is_null() {
        return;
    }
    unsafe {
        (*tcb).state = ThreadState::Inactive as u8;
    }
}

pub unsafe fn resume(tcb: *mut Tcb) {
    if tcb.is_null() {
        return;
    }
    unsafe {
        (*tcb).state = ThreadState::Restart as u8;
    }
}

pub unsafe fn set_priority(tcb: *mut Tcb, prio: u8) {
    if tcb.is_null() {
        return;
    }
    unsafe {
        (*tcb).priority = prio;
    }
}

pub unsafe fn set_mcp(tcb: *mut Tcb, mcp: u8) {
    if tcb.is_null() {
        return;
    }
    unsafe {
        (*tcb).mcp = mcp;
    }
}

pub unsafe fn set_flags(tcb: *mut Tcb, flags: u32) {
    if tcb.is_null() {
        return;
    }
    unsafe {
        (*tcb).flags = flags;
    }
}

pub unsafe fn set_tls_base(tcb: *mut Tcb, tls_base: u64) {
    if tcb.is_null() {
        return;
    }
    unsafe {
        (*tcb).context.regs[crate::arch::riscv64::trap::reg::TP] = tls_base;
    }
}

pub unsafe fn bind_notification(tcb: *mut Tcb, ntfn_kva: u64) {
    if tcb.is_null() {
        return;
    }
    unsafe {
        (*tcb).bound_notification = ntfn_kva;
    }
}

pub unsafe fn unbind_notification(tcb: *mut Tcb) {
    if tcb.is_null() {
        return;
    }
    unsafe {
        (*tcb).bound_notification = 0;
    }
}

/// Wipe a TCB on destruction. Called from `finalize_cap(Thread)`.
///
/// We don't yet maintain a ready queue, but we still drop the bound
/// notification and clear the state byte so a stale pointer to this
/// memory (post-Retype) doesn't look "runnable" if some future code
/// path tries to schedule it during the in-flight Revoke.
pub unsafe fn finalize(tcb: *mut Tcb) {
    if tcb.is_null() {
        return;
    }
    unsafe {
        (*tcb).bound_notification = 0;
        (*tcb).queue_next = 0;
        (*tcb).queue_prev = 0;
        (*tcb).waiting_on = 0;
        (*tcb).state = ThreadState::Inactive as u8;
    }
}
