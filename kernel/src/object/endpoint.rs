//! Kernel-side Endpoint object.
//!
//! Lives in the 16-byte (`seL4_EndpointBits = 4`) region the user
//! retypes from an Untyped via `Untyped_Retype(seL4_EndpointObject)`.
//! That alignment guarantee makes us safe to treat the cap's pointer as
//! `*mut Endpoint`.
//!
//! Layout follows the C kernel's `endpoint_t` from
//! `kernel/include/object/structures.h`:
//!
//! ```c
//! struct endpoint {
//!     uint64_t epQueue_head_state;  // queue head ptr | state in low 2 bits
//!     uint64_t epQueue_tail;
//! };
//! ```
//!
//! The TCB wait list is doubly-linked through `Tcb.queue_{next,prev}`
//! (the same fields the runqueue uses) — safe because a TCB is either
//! runnable (in a runqueue bin) or blocked-on-EP (in an EP's wait list),
//! never both at once.

#![allow(dead_code)]

use crate::object::tcb::{self, Tcb};
use crate::object::wait_queue_lock::{self, WaitQueueLockGuard};

/// 2-bit Endpoint state, encoded in the low bits of `head_state`.
#[repr(u8)]
#[derive(Copy, Clone, PartialEq, Eq, Debug)]
pub enum EpState {
    /// No waiting senders or receivers.
    Idle = 0,
    /// Queue holds blocked senders. A receiver arriving at this state
    /// will pair with the head sender (rendezvous).
    Sending = 1,
    /// Queue holds blocked receivers. A sender arriving at this state
    /// will pair with the head receiver (rendezvous).
    Receiving = 2,
}

/// Mask for the state bits embedded in `head_state`.
const STATE_MASK: u64 = 0x3;
const HEAD_MASK: u64 = !STATE_MASK;

#[repr(C)]
pub struct Endpoint {
    /// `(head_ptr & !0x3) | (state & 0x3)`. The TCB pointers are 2 KiB
    /// aligned (`seL4_TCBBits = 11`) so the low 11 bits are always
    /// zero — using the bottom 2 is safe.
    head_state: u64,
    /// PSpace KVA of the last waiter, or 0.
    tail: u64,
}

// 4 bits ⇒ 16 bytes per Endpoint slab.
const _: () = {
    assert!(core::mem::size_of::<Endpoint>() == 16);
};

impl Endpoint {
    pub const fn zero() -> Self {
        Endpoint {
            head_state: 0,
            tail: 0,
        }
    }

    #[inline]
    fn state(&self) -> EpState {
        match self.head_state & STATE_MASK {
            1 => EpState::Sending,
            2 => EpState::Receiving,
            _ => EpState::Idle,
        }
    }

    #[inline]
    fn head(&self) -> *mut Tcb {
        (self.head_state & HEAD_MASK) as *mut Tcb
    }

    #[inline]
    fn tail_ptr(&self) -> *mut Tcb {
        self.tail as *mut Tcb
    }

    #[inline]
    fn set_head_state(&mut self, head: *mut Tcb, state: EpState) {
        self.head_state = ((head as u64) & HEAD_MASK) | (state as u64);
    }

    #[inline]
    fn set_tail(&mut self, tail: *mut Tcb) {
        self.tail = tail as u64;
    }
}

/// Initialise a freshly-retyped 16-byte Endpoint slab. `Untyped_Retype`
/// already zeroed the memory, so all fields land at Idle / null.
pub unsafe fn init(ep_kva: u64) {
    crate::kernel::smp::debug_assert_kernel_lock_held();
    let p = ep_kva as *mut Endpoint;
    unsafe {
        (*p).head_state = 0;
        (*p).tail = 0;
    }
}

#[inline]
pub(crate) unsafe fn lock_queue(ep: *const Endpoint) -> WaitQueueLockGuard {
    wait_queue_lock::lock(ep.cast())
}

/// Append `tcb` to the tail of `ep`'s wait list, updating state. Caller
/// is responsible for marking the TCB as blocked + dequeueing it from
/// the runqueue before calling this.
pub unsafe fn enqueue_waiter(ep: *mut Endpoint, tcb: *mut Tcb, state: EpState) {
    if ep.is_null() || tcb.is_null() {
        return;
    }
    let _guard = unsafe { lock_queue(ep) };
    unsafe {
        enqueue_waiter_locked(ep, tcb, state);
    }
}

pub(crate) unsafe fn enqueue_waiter_locked(ep: *mut Endpoint, tcb: *mut Tcb, state: EpState) {
    if ep.is_null() || tcb.is_null() {
        return;
    }
    unsafe {
        let tcb_prio = tcb::priority_snapshot(tcb);
        let mut before = (*ep).tail_ptr();
        let mut after = core::ptr::null_mut::<Tcb>();
        while !before.is_null() && tcb_prio > tcb::priority_snapshot(before) {
            after = before;
            before = tcb::wait_queue_links_snapshot(before).0;
        }

        if before.is_null() {
            (*ep).set_head_state(tcb, state);
        } else {
            tcb::set_wait_queue_next(before, tcb);
        }

        if after.is_null() {
            (*ep).set_tail(tcb);
        } else {
            tcb::set_wait_queue_prev(after, tcb);
        }

        tcb::set_wait_queue_links(tcb, before, after);
    }
}

/// Pop the head of `ep`'s wait list. Returns null if the list was
/// empty. Doesn't touch the popped TCB's `state` field — caller must
/// transition it (typically: Running + re-enqueue into the runqueue).
pub unsafe fn pop_head(ep: *mut Endpoint) -> *mut Tcb {
    if ep.is_null() {
        return core::ptr::null_mut();
    }
    let _guard = unsafe { lock_queue(ep) };
    unsafe { pop_head_locked(ep) }
}

pub(crate) unsafe fn pop_head_locked(ep: *mut Endpoint) -> *mut Tcb {
    if ep.is_null() {
        return core::ptr::null_mut();
    }
    unsafe {
        let head = (*ep).head();
        if head.is_null() {
            return core::ptr::null_mut();
        }
        let next = tcb::wait_queue_next_snapshot(head);
        tcb::clear_wait_queue_links(head);
        if next.is_null() {
            (*ep).set_head_state(core::ptr::null_mut(), EpState::Idle);
            (*ep).set_tail(core::ptr::null_mut());
        } else {
            tcb::set_wait_queue_prev(next, core::ptr::null_mut());
            // Preserve the existing state (Sending or Receiving) since
            // the wait list still holds peers of the same flavour.
            let st = (*ep).state();
            (*ep).set_head_state(next, st);
        }
        head
    }
}

/// Caller must hold this Endpoint's queue lock, or an ordered pair lock that
/// includes it.
pub(crate) unsafe fn pop_sender_locked(ep: *mut Endpoint) -> *mut Tcb {
    unsafe { pop_waiter_if_state_locked(ep, EpState::Sending) }
}

/// Caller must hold this Endpoint's queue lock, or an ordered pair lock that
/// includes it.
pub(crate) unsafe fn pop_receiver_locked(ep: *mut Endpoint) -> *mut Tcb {
    unsafe { pop_waiter_if_state_locked(ep, EpState::Receiving) }
}

unsafe fn pop_waiter_if_state_locked(ep: *mut Endpoint, state: EpState) -> *mut Tcb {
    if ep.is_null() {
        return core::ptr::null_mut();
    }
    unsafe {
        if (*ep).state() != state {
            return core::ptr::null_mut();
        }
        pop_head_locked(ep)
    }
}

/// Remove an arbitrary `tcb` from `ep`'s wait list. Used by
/// `finalize` and by Suspend on a blocked TCB.
pub unsafe fn remove_waiter(ep: *mut Endpoint, tcb: *mut Tcb) {
    if ep.is_null() || tcb.is_null() {
        return;
    }
    let _guard = unsafe { lock_queue(ep) };
    unsafe {
        if !contains_waiter_locked(ep, tcb) {
            return;
        }
        remove_waiter_locked(ep, tcb);
    }
}

pub(crate) unsafe fn remove_waiter_locked(ep: *mut Endpoint, tcb: *mut Tcb) {
    if ep.is_null() || tcb.is_null() {
        return;
    }
    unsafe {
        let (prev, next) = tcb::wait_queue_links_snapshot(tcb);
        if !prev.is_null() {
            tcb::set_wait_queue_next(prev, next);
        } else if (*ep).head() == tcb {
            // tcb was head — promote next.
            let st = (*ep).state();
            (*ep).set_head_state(next, st);
        }
        if !next.is_null() {
            tcb::set_wait_queue_prev(next, prev);
        } else if (*ep).tail_ptr() == tcb {
            (*ep).set_tail(prev);
        }
        tcb::clear_wait_queue_links(tcb);
        if (*ep).head().is_null() {
            (*ep).set_head_state(core::ptr::null_mut(), EpState::Idle);
            (*ep).set_tail(core::ptr::null_mut());
        }
    }
}

unsafe fn contains_waiter_locked(ep: *mut Endpoint, tcb: *mut Tcb) -> bool {
    if ep.is_null() || tcb.is_null() {
        return false;
    }
    unsafe {
        let mut cur = (*ep).head();
        while !cur.is_null() {
            if cur == tcb {
                return true;
            }
            cur = tcb::wait_queue_next_snapshot(cur);
        }
    }
    false
}

unsafe fn waiter_matches_endpoint_locked(ep: *mut Endpoint, tcb: *mut Tcb, state: EpState) -> bool {
    if state == EpState::Idle || unsafe { !contains_waiter_locked(ep, tcb) } {
        return false;
    }
    match state {
        EpState::Sending => tcb::waiter_matches_endpoint(tcb, ep as u64, true),
        EpState::Receiving => tcb::waiter_matches_endpoint(tcb, ep as u64, false),
        EpState::Idle => false,
    }
}

pub unsafe fn reorder_waiter(ep: *mut Endpoint, tcb: *mut Tcb) {
    if ep.is_null() || tcb.is_null() {
        return;
    }
    let _guard = unsafe { lock_queue(ep) };
    unsafe {
        let state = (*ep).state();
        if !waiter_matches_endpoint_locked(ep, tcb, state) {
            return;
        }
        remove_waiter_locked(ep, tcb);
        enqueue_waiter_locked(ep, tcb, state);
    }
}

unsafe fn append_detached(head: &mut *mut Tcb, tail: &mut *mut Tcb, tcb: *mut Tcb) {
    unsafe {
        tcb::set_wait_queue_links(tcb, *tail, core::ptr::null_mut());
        if (*tail).is_null() {
            *head = tcb;
        } else {
            tcb::set_wait_queue_next(*tail, tcb);
        }
        *tail = tcb;
    }
}

unsafe fn take_all_locked(ep: *mut Endpoint) -> *mut Tcb {
    unsafe {
        let head = (*ep).head();
        (*ep).set_head_state(core::ptr::null_mut(), EpState::Idle);
        (*ep).set_tail(core::ptr::null_mut());
        head
    }
}

/// `cancelBadgedSends(ep, badge)` (C kernel name): traverse `ep`'s
/// wait list and cancel every blocked sender whose `sender_badge` matches
/// `badge`. Non-matching senders, and any blocked receivers, are left
/// in place. Matching normal IPC senders move to `Restart` and re-enter the
/// runqueue; matching fault senders are left inactive with their pending fault
/// preserved, mirroring seL4 `restart_thread_if_no_fault`.
pub unsafe fn cancel_badged_sends(ep: *mut Endpoint, badge: u64) {
    if ep.is_null() {
        return;
    }
    let mut wake_head: *mut Tcb = core::ptr::null_mut();
    let mut wake_tail: *mut Tcb = core::ptr::null_mut();
    unsafe {
        {
            let _guard = lock_queue(ep);
            // Only meaningful if EP is currently holding senders.
            if (*ep).state() != EpState::Sending {
                return;
            }

            // Snapshot the queue and rebuild non-matching waiters under the
            // EP lock. Matching senders are woken after the lock is released.
            let head = take_all_locked(ep);
            let mut new_head: *mut Tcb = core::ptr::null_mut();
            let mut new_tail: *mut Tcb = core::ptr::null_mut();

            let mut cur = head;
            while !cur.is_null() {
                let next = tcb::wait_queue_next_snapshot(cur);
                tcb::clear_wait_queue_links(cur);
                if tcb::sender_badge_snapshot(cur) == badge {
                    append_detached(&mut wake_head, &mut wake_tail, cur);
                } else {
                    append_detached(&mut new_head, &mut new_tail, cur);
                }
                cur = next;
            }

            if !new_head.is_null() {
                (*ep).set_head_state(new_head, EpState::Sending);
                (*ep).set_tail(new_tail);
            }
        }

        let mut cur = wake_head;
        while !cur.is_null() {
            let (next, runnable) = tcb::cancel_endpoint_waiter(cur, None);
            if runnable {
                tcb::enqueue(cur);
            }
            cur = next;
        }
    }
}

/// Drain `ep`'s wait list on destruction: wake every blocked TCB so
/// the cap-revoke teardown doesn't leak threads. Normal IPC waiters are
/// restarted and requeued; fault senders become inactive with their pending
/// fault preserved because the handler endpoint send was aborted.
pub unsafe fn finalize(ep: *mut Endpoint) {
    crate::kernel::smp::debug_assert_kernel_lock_held();
    if ep.is_null() {
        return;
    }
    let head = {
        let _guard = unsafe { lock_queue(ep) };
        unsafe { take_all_locked(ep) }
    };
    let mut cur = head;
    while !cur.is_null() {
        unsafe {
            let (next, runnable) = tcb::cancel_endpoint_waiter(cur, None);
            if runnable {
                crate::object::tcb::enqueue(cur);
            }
            cur = next;
        }
    }
}
