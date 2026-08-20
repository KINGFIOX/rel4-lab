//! Kernel-side Endpoint object.
//!
//! Lives in the 16-byte (`seL4_EndpointBits = 4`) region the user
//! retypes from an Untyped via `Untyped_Retype(seL4_EndpointObject)`.
//! That alignment guarantee makes the cap's pointer a valid [`EndpointRef`].
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
//! The wait list is doubly linked through the [`Links`] embedded in each TCB
//! (the same links the runqueue uses) — sound because a TCB is either
//! runnable, or blocked on one wait object, never both at once. The head and
//! tail live in the two words above, so `Endpoint` implements
//! [`QueueEnds`] and the linking itself is `ktypes::list`'s job.

#![allow(dead_code)]

use crate::ktypes::list::{self, QueueEnds};
use crate::ktypes::objref::{ObjRef, OptObjRefExt};
use crate::object::tcb::{Tcb, TcbRef};

/// Handle for an endpoint object.
pub type EndpointRef = ObjRef<Endpoint>;

/// 2-bit Endpoint state, encoded in the low bits of `head_state`.
#[repr(u8)]
#[derive(Copy, Clone, PartialEq, Eq, Debug, Default)]
pub enum EpState {
    /// No waiting senders or receivers.
    #[default]
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
    assert!(size_of::<Endpoint>() == 16);
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
    fn set_state(&mut self, state: EpState) {
        self.head_state = (self.head_state & HEAD_MASK) | (state as u64);
    }

    /// Reset to the empty, idle endpoint, which is also what a freshly
    /// retyped slab looks like.
    #[inline]
    fn clear(&mut self) {
        self.head_state = 0;
        self.tail = 0;
    }
}

impl QueueEnds<Tcb> for Endpoint {
    #[inline]
    fn head(&self) -> Option<TcbRef> {
        // SAFETY: the field only ever holds an address this module stored
        // from a live `TcbRef`, with the low state bits masked back off.
        unsafe { ObjRef::from_kva(self.head_state & HEAD_MASK) }
    }

    #[inline]
    fn tail(&self) -> Option<TcbRef> {
        // SAFETY: as `head`.
        unsafe { ObjRef::from_kva(self.tail) }
    }

    #[inline]
    fn set_head(&mut self, head: Option<TcbRef>) {
        // TCB slabs are 2 KiB aligned, so storing the address never disturbs
        // the state bits.
        self.head_state = (head.kva_or_zero() & HEAD_MASK) | (self.head_state & STATE_MASK);
    }

    #[inline]
    fn set_tail(&mut self, tail: Option<TcbRef>) {
        self.tail = tail.kva_or_zero();
    }
}

/// Initialise a freshly-retyped 16-byte Endpoint slab. `Untyped_Retype`
/// already zeroed the memory, so all fields land at Idle / null.
///
/// # Safety
/// `ep_kva` must be the base of a zeroed, 16-byte-aligned slab that the
/// caller has just retyped into an Endpoint object.
pub unsafe fn init(ep_kva: u64) {
    crate::kernel::smp::debug_assert_kernel_lock_held();
    // SAFETY: forwarded to the caller; an all-zero slab is a valid Endpoint.
    let ep: EndpointRef = unsafe { ObjRef::from_kva_unchecked(ep_kva) };
    ep.with_mut(Endpoint::clear);
}

impl EndpointRef {
    #[inline]
    pub fn state(self) -> EpState {
        self.with(Endpoint::state)
    }

    #[inline]
    pub fn head(self) -> Option<TcbRef> {
        self.with(QueueEnds::head)
    }

    /// Append `tcb` to the tail of the wait list, moving the endpoint into
    /// `state`. The caller marks the TCB blocked and takes it off the
    /// runqueue first.
    pub fn enqueue_waiter(self, tcb: TcbRef, state: EpState) {
        self.with_mut(|ep| {
            ep.set_state(state);
            list::push_back(ep, tcb);
        });
    }

    /// Remove and return the first waiter, leaving its thread state alone:
    /// the caller transitions it (typically to Running plus a re-enqueue).
    pub fn pop_head(self) -> Option<TcbRef> {
        let popped = self.with_mut(list::pop_front);
        // An emptied queue goes back to Idle so a later sender does not think
        // there is still a peer to pair with.
        self.with_mut(|ep| {
            if QueueEnds::head(ep).is_none() {
                ep.clear();
            }
        });
        popped
    }

    /// Pop the first waiter only if the endpoint is holding waiters of the
    /// given flavour.
    pub fn pop_waiter_if_state(self, state: EpState) -> Option<TcbRef> {
        if self.state() != state {
            return None;
        }
        self.pop_head()
    }

    /// Pop a blocked sender, if any is queued.
    pub fn pop_sender(self) -> Option<TcbRef> {
        self.pop_waiter_if_state(EpState::Sending)
    }

    /// Pop a blocked receiver, if any is queued.
    pub fn pop_receiver(self) -> Option<TcbRef> {
        self.pop_waiter_if_state(EpState::Receiving)
    }

    /// Remove an arbitrary waiter. Used by `finalize` and by Suspend on a
    /// blocked TCB.
    pub fn remove_waiter(self, tcb: TcbRef) {
        self.with_mut(|ep| {
            if !list::contains(ep, tcb) {
                return;
            }
            list::remove(ep, tcb);
            if QueueEnds::head(ep).is_none() {
                ep.clear();
            }
        });
    }

    /// Move `tcb` to the tail of the wait list, if it really is waiting here.
    pub fn reorder_waiter(self, tcb: TcbRef) {
        let state = self.state();
        if state == EpState::Idle || !self.contains_waiter(tcb) {
            return;
        }
        if !tcb.waits_on_endpoint(self, state == EpState::Sending) {
            return;
        }
        self.remove_waiter(tcb);
        self.enqueue_waiter(tcb, state);
    }

    pub fn contains_waiter(self, tcb: TcbRef) -> bool {
        self.with(|ep| list::contains(ep, tcb))
    }

    /// Detach the whole wait list and reset the endpoint to Idle, returning
    /// the old head so the caller can walk it.
    fn take_all(self) -> Option<TcbRef> {
        self.with_mut(|ep| {
            let head = QueueEnds::head(ep);
            ep.clear();
            head
        })
    }

    /// `cancelBadgedSends(ep, badge)`: cancel every blocked sender whose
    /// badge matches. Non-matching senders and any blocked receivers stay
    /// queued. Matching normal IPC senders move to `Restart` and re-enter the
    /// runqueue; matching fault senders are left inactive, mirroring seL4
    /// `restart_thread_if_no_fault`.
    pub fn cancel_badged_sends(self, badge: u64) {
        // Only meaningful if the endpoint is currently holding senders.
        if self.state() != EpState::Sending {
            return;
        }

        // Rebuild the list of non-matching waiters, collecting the matching
        // ones to wake after the endpoint is consistent again.
        let mut keep = list::Queue::<Tcb>::new();
        let mut wake = list::Queue::<Tcb>::new();
        let mut next = self.take_all();
        while let Some(waiter) = next {
            next = list::next_of(waiter);
            list::clear_links(waiter);
            if waiter.sender_badge() == badge {
                list::push_back(&mut wake, waiter);
            } else {
                list::push_back(&mut keep, waiter);
            }
        }

        if let Some(head) = keep.head() {
            self.with_mut(|ep| {
                ep.set_state(EpState::Sending);
                ep.set_head(Some(head));
                ep.set_tail(keep.tail());
            });
        }

        while let Some(waiter) = list::pop_front(&mut wake) {
            let (_, runnable) = waiter.cancel_endpoint_waiter(None);
            if runnable {
                waiter.enqueue();
            }
        }
    }

    /// Drain the wait list on destruction so the cap-revoke teardown does not
    /// leak threads. Normal IPC waiters are restarted and requeued; fault
    /// senders become inactive with their pending fault preserved, because the
    /// handler endpoint send was aborted.
    pub fn finalize(self) {
        crate::kernel::smp::debug_assert_kernel_lock_held();
        let mut next = self.take_all();
        while let Some(waiter) = next {
            let (following, runnable) = waiter.cancel_endpoint_waiter(None);
            next = following;
            if runnable {
                waiter.enqueue();
            }
        }
    }
}
