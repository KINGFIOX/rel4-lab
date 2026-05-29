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

use core::cell::UnsafeCell;
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

/// Replace `CURRENT_TCB`. Returns the previous pointer. Also refreshes
/// the legacy `api::thread::CURRENT` view so cap lookups and IPC
/// accesses in the syscall slow path follow whichever TCB the
/// scheduler last picked.
#[inline]
pub fn set_current(tcb: *mut Tcb) -> *mut Tcb {
    let prev = CURRENT_TCB.swap(tcb, Ordering::AcqRel);
    unsafe { crate::api::thread::refresh_from_tcb(tcb) };
    prev
}

// ---- Ready-queue runqueues ------------------------------------------------
//
// `CONFIG_NUM_PRIORITIES = 256` per `kernel/include/configurations/gen_config.h`.
// One doubly-linked list per priority, all backed by `Tcb.queue_{next,prev}`
// (stored as raw u64 ptrs because `Tcb` lives in user-controlled memory
// and we want the field offsets to stay byte-identical across the C ↔ Rust
// boundary).
//
// Single-hart, no preemption ⇒ no concurrency, so plain `UnsafeCell` is
// safe behind the kernel's "interrupts off during trap" guarantee.

pub const NUM_PRIORITIES: usize = 256;

#[repr(C)]
#[derive(Copy, Clone)]
struct Queue {
    head: *mut Tcb,
    tail: *mut Tcb,
}

#[repr(transparent)]
struct QueueCell(UnsafeCell<Queue>);
unsafe impl Sync for QueueCell {}

static RUNQUEUES: [QueueCell; NUM_PRIORITIES] = {
    const EMPTY: QueueCell = QueueCell(UnsafeCell::new(Queue {
        head: null_mut(),
        tail: null_mut(),
    }));
    [EMPTY; NUM_PRIORITIES]
};

/// Per-priority "any ready TCB?" summary. We keep a 256-bit bitmap in 4
/// u64 words; `schedule()` does a constant-time scan from highest to
/// lowest priority by walking from word 3 down to word 0.
static READY_BITMAP: [core::sync::atomic::AtomicU64; 4] = [
    core::sync::atomic::AtomicU64::new(0),
    core::sync::atomic::AtomicU64::new(0),
    core::sync::atomic::AtomicU64::new(0),
    core::sync::atomic::AtomicU64::new(0),
];

#[inline]
fn rq(prio: usize) -> *mut Queue {
    debug_assert!(prio < NUM_PRIORITIES);
    RUNQUEUES[prio].0.get()
}

#[inline]
fn set_ready_bit(prio: usize) {
    READY_BITMAP[prio / 64].fetch_or(1u64 << (prio % 64), Ordering::AcqRel);
}

#[inline]
fn clear_ready_bit(prio: usize) {
    READY_BITMAP[prio / 64].fetch_and(!(1u64 << (prio % 64)), Ordering::AcqRel);
}

/// Append `tcb` to the tail of its priority's queue. No-op if already
/// linked (i.e. `queue_next` or `queue_prev` non-zero, or `head == tcb`).
pub unsafe fn enqueue(tcb: *mut Tcb) {
    if tcb.is_null() {
        return;
    }
    let prio = unsafe { (*tcb).priority as usize };
    let q = rq(prio);
    let tcb_u = tcb as u64;
    unsafe {
        // Already on a queue?
        if (*tcb).queue_next != 0 || (*tcb).queue_prev != 0 || (*q).head == tcb {
            return;
        }
        (*tcb).queue_next = 0;
        (*tcb).queue_prev = (*q).tail as u64;
        if (*q).tail.is_null() {
            (*q).head = tcb;
        } else {
            (*((*q).tail)).queue_next = tcb_u;
        }
        (*q).tail = tcb;
    }
    set_ready_bit(prio);
}

/// Unlink `tcb` from its priority's queue. No-op if not currently linked.
pub unsafe fn dequeue(tcb: *mut Tcb) {
    if tcb.is_null() {
        return;
    }
    let prio = unsafe { (*tcb).priority as usize };
    let q = rq(prio);
    unsafe {
        let prev = (*tcb).queue_prev as *mut Tcb;
        let next = (*tcb).queue_next as *mut Tcb;
        let was_linked = !prev.is_null() || !next.is_null() || (*q).head == tcb;
        if !was_linked {
            return;
        }
        if !prev.is_null() {
            (*prev).queue_next = next as u64;
        } else {
            (*q).head = next;
        }
        if !next.is_null() {
            (*next).queue_prev = prev as u64;
        } else {
            (*q).tail = prev;
        }
        (*tcb).queue_next = 0;
        (*tcb).queue_prev = 0;

        if (*q).head.is_null() {
            clear_ready_bit(prio);
        }
    }
}

/// Move `tcb` to the tail of its own priority's queue. Used by
/// `seL4_Yield` to surrender the CPU to a same-priority peer.
pub unsafe fn rotate_to_tail(tcb: *mut Tcb) {
    if tcb.is_null() {
        return;
    }
    let prio = unsafe { (*tcb).priority as usize };
    let q = rq(prio);
    unsafe {
        if (*q).head == tcb && (*q).tail == tcb {
            return; // singleton, nothing to do
        }
        dequeue(tcb);
        enqueue(tcb);
    }
}

/// Pick the highest-priority ready TCB, or `null` if all queues empty.
///
/// O(1) on the 256 priorities: scan the 4-word ready bitmap from MSB
/// down, then `head` of the first bin we find.
pub fn schedule() -> *mut Tcb {
    for word_idx in (0..4).rev() {
        let bits = READY_BITMAP[word_idx].load(Ordering::Acquire);
        if bits == 0 {
            continue;
        }
        // Highest set bit in `bits` ⇒ highest priority in this word.
        let bit = 63 - bits.leading_zeros() as usize;
        let prio = word_idx * 64 + bit;
        let q = rq(prio);
        let head = unsafe { (*q).head };
        if !head.is_null() {
            return head;
        }
    }
    null_mut()
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
    pub receiver_can_grant: u8,

    /// "Implicit reply target" used by the pre-MCS Call/Reply pattern.
    /// When a sender's `seL4_Call` rendezvous with a receiver, the
    /// receiver records the sender's TCB KVA here so its subsequent
    /// `seL4_Reply` knows who to wake. 0 means "no caller".
    pub caller: u64,
    pub caller_can_grant: u8,
    pub reply_slot: u64,

    /// Badge from the cap used to Send / Call. Stashed when a sender
    /// blocks on an Endpoint so the eventual receiver can read it back
    /// without re-walking the sender's CSpace.
    pub sender_badge: u64,
    pub sender_can_grant: u8,
    pub sender_can_grant_reply: u8,
    /// `1` iff the queued-up Send was originally a `seL4_Call`. The
    /// receiver consults this on rendezvous to decide whether to put
    /// the sender into `BlockedOnReply` (Call) or wake it directly
    /// (plain Send).
    pub sender_is_call: u8,

    /// `1` iff the queued-up Call is a synthetic fault IPC. Fault IPC
    /// must not borrow the faulting thread's message registers: the
    /// handler reply restarts the trapped instruction, so the original
    /// user register file has to survive intact.
    pub sender_is_fault: u8,
    pub fault_label: u64,
    pub fault_len: u64,
    pub fault_mrs: [u64; 16],

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
                fregs: [0; 32],
                fcsr: 0,
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
            receiver_can_grant: 0,
            caller: 0,
            caller_can_grant: 0,
            reply_slot: 0,
            sender_badge: 0,
            sender_can_grant: 0,
            sender_can_grant_reply: 0,
            sender_is_call: 0,
            sender_is_fault: 0,
            fault_label: 0,
            fault_len: 0,
            fault_mrs: [0; 16],
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
    if p == 0 { null_mut() } else { p as *mut Tcb }
}

/// Initialise a freshly-retyped 2 KiB TCB slab.
///
/// `Untyped_Retype` already zeroed the memory; we only stamp the bits
/// where 0 isn't the right resting value (currently just sstatus so a
/// future `restore_user_context` returns to U-mode with interrupts and
/// the FPU enabled.
pub unsafe fn init(tcb_kva: u64) {
    let t = tcb_kva as *mut Tcb;
    // sstatus.SPIE = 1 -> sret re-enables interrupts in U-mode.
    // sstatus.SPP  = 0 -> sret enters U-mode (already 0).
    // sstatus.FS   = Dirty -> user floating-point instructions are legal.
    unsafe {
        (*t).state = ThreadState::Inactive as u8;
        (*t).context.sstatus = crate::arch::riscv64::trap::USER_SSTATUS;
    }
}

/// Detach `tcb` from any Endpoint or Notification wait list it might be
/// queued on (because of a prior blocking Send / Recv / Call / Wait).
/// Safe to call on a TCB that isn't waiting on anything — it
/// short-circuits on `waiting_on == 0`.
///
/// Dispatch on `tcb.state` so we route to the right object: BlockedOn{Receive,
/// Send, Reply} → Endpoint; BlockedOnNotification → Notification.
unsafe fn unlink_from_wait_object(tcb: *mut Tcb) {
    if tcb.is_null() {
        return;
    }
    let obj = unsafe { (*tcb).waiting_on };
    if obj == 0 {
        return;
    }
    let st = unsafe { (*tcb).state };
    unsafe {
        if st == ThreadState::BlockedOnNotification as u8 {
            let ntfn = obj as *mut crate::object::notification::Notification;
            crate::object::notification::remove_waiter(ntfn, tcb);
        } else {
            let ep = obj as *mut crate::object::endpoint::Endpoint;
            crate::object::endpoint::remove_waiter(ep, tcb);
        }
        (*tcb).waiting_on = 0;
        (*tcb).receiver_can_grant = 0;
        (*tcb).sender_badge = 0;
        (*tcb).sender_can_grant = 0;
        (*tcb).sender_can_grant_reply = 0;
        (*tcb).sender_is_call = 0;
    }
}

/// Per-invocation primitives. They mark the TCB runnable / non-runnable
/// and update the global ready-queue accordingly. The actual CPU swap
/// happens at the next `kernel_exit()` boundary (top of `restore_user_context`).
pub unsafe fn suspend(tcb: *mut Tcb) {
    if tcb.is_null() {
        return;
    }
    unsafe {
        // A suspended TCB must leave any EP wait list it's queued on,
        // otherwise the EP would later try to `pop_head` a TCB whose
        // backing slab might be reused.
        unlink_from_wait_object(tcb);
        dequeue(tcb);
        let reply_slot = (*tcb).reply_slot as *mut crate::object::cnode::Cte;
        if !reply_slot.is_null() {
            (*reply_slot).cap = crate::object::cap::Cap::null();
            (*tcb).reply_slot = 0;
        }
        (*tcb).state = ThreadState::Inactive as u8;
    }
}

pub unsafe fn resume(tcb: *mut Tcb) {
    if tcb.is_null() {
        return;
    }
    unsafe {
        (*tcb).state = ThreadState::Running as u8;
        enqueue(tcb);
    }
}

pub unsafe fn set_priority(tcb: *mut Tcb, prio: u8) {
    if tcb.is_null() {
        return;
    }
    unsafe {
        let was_running = (*tcb).state == ThreadState::Running as u8;
        if was_running {
            dequeue(tcb);
        }
        (*tcb).priority = prio;
        if was_running {
            enqueue(tcb);
        }
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
    if tcb.is_null() || ntfn_kva == 0 {
        return;
    }
    unsafe {
        // First clear the TCB's existing binding (if any) — symmetric
        // with `unbind_notification`, so a re-bind doesn't leave a
        // stale back-pointer in the old notification.
        let prev_ntfn = (*tcb).bound_notification;
        if prev_ntfn != 0 {
            let p = prev_ntfn as *mut crate::object::notification::Notification;
            (*p).set_bound_tcb(0);
        }
        (*tcb).bound_notification = ntfn_kva;
        let ntfn_ptr = ntfn_kva as *mut crate::object::notification::Notification;
        (*ntfn_ptr).set_bound_tcb(tcb as u64);
    }
}

pub unsafe fn unbind_notification(tcb: *mut Tcb) {
    if tcb.is_null() {
        return;
    }
    unsafe {
        let ntfn_kva = (*tcb).bound_notification;
        if ntfn_kva != 0 {
            let p = ntfn_kva as *mut crate::object::notification::Notification;
            (*p).set_bound_tcb(0);
        }
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
        // 1. Detach from any EP wait list. Crucial: a TCB whose slab
        //    is about to be recycled by Untyped_Revoke cannot remain
        //    queued on an Endpoint — that would leave the EP's
        //    `pop_head` returning a freed pointer.
        unlink_from_wait_object(tcb);
        // 2. Drop our binding so the notification stops believing
        //    we're still its owner (symmetric with unbind_notification).
        unbind_notification(tcb);
        // 3. Remove from the runqueue so the scheduler can't pick us.
        dequeue(tcb);
        (*tcb).queue_next = 0;
        (*tcb).queue_prev = 0;
        (*tcb).caller = 0;
        let reply_slot = (*tcb).reply_slot as *mut crate::object::cnode::Cte;
        if !reply_slot.is_null() {
            (*reply_slot).cap = crate::object::cap::Cap::null();
            (*tcb).reply_slot = 0;
        }
        (*tcb).state = ThreadState::Inactive as u8;
    }
}
