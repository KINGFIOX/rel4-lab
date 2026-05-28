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

use crate::object::tcb::Tcb;

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
    pub head_state: u64,
    /// PSpace KVA of the last waiter, or 0.
    pub tail: u64,
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
    pub fn state(&self) -> EpState {
        match self.head_state & STATE_MASK {
            1 => EpState::Sending,
            2 => EpState::Receiving,
            _ => EpState::Idle,
        }
    }

    #[inline]
    pub fn head(&self) -> *mut Tcb {
        (self.head_state & HEAD_MASK) as *mut Tcb
    }

    #[inline]
    pub fn tail_ptr(&self) -> *mut Tcb {
        self.tail as *mut Tcb
    }

    #[inline]
    pub fn set_head_state(&mut self, head: *mut Tcb, state: EpState) {
        self.head_state = ((head as u64) & HEAD_MASK) | (state as u64);
    }

    #[inline]
    pub fn set_tail(&mut self, tail: *mut Tcb) {
        self.tail = tail as u64;
    }
}

/// Initialise a freshly-retyped 16-byte Endpoint slab. `Untyped_Retype`
/// already zeroed the memory, so all fields land at Idle / null.
pub unsafe fn init(ep_kva: u64) {
    let p = ep_kva as *mut Endpoint;
    unsafe {
        (*p).head_state = 0;
        (*p).tail = 0;
    }
}

/// Append `tcb` to the tail of `ep`'s wait list, updating state. Caller
/// is responsible for marking the TCB as blocked + dequeueing it from
/// the runqueue before calling this.
pub unsafe fn enqueue_waiter(ep: *mut Endpoint, tcb: *mut Tcb, state: EpState) {
    if ep.is_null() || tcb.is_null() {
        return;
    }
    unsafe {
        (*tcb).queue_next = 0;
        let old_tail = (*ep).tail_ptr();
        (*tcb).queue_prev = old_tail as u64;
        if old_tail.is_null() {
            (*ep).set_head_state(tcb, state);
        } else {
            (*old_tail).queue_next = tcb as u64;
        }
        (*ep).set_tail(tcb);
    }
}

/// Pop the head of `ep`'s wait list. Returns null if the list was
/// empty. Doesn't touch the popped TCB's `state` field — caller must
/// transition it (typically: Running + re-enqueue into the runqueue).
pub unsafe fn pop_head(ep: *mut Endpoint) -> *mut Tcb {
    if ep.is_null() {
        return core::ptr::null_mut();
    }
    unsafe {
        let head = (*ep).head();
        if head.is_null() {
            return core::ptr::null_mut();
        }
        let next = (*head).queue_next as *mut Tcb;
        (*head).queue_next = 0;
        (*head).queue_prev = 0;
        if next.is_null() {
            (*ep).set_head_state(core::ptr::null_mut(), EpState::Idle);
            (*ep).set_tail(core::ptr::null_mut());
        } else {
            (*next).queue_prev = 0;
            // Preserve the existing state (Sending or Receiving) since
            // the wait list still holds peers of the same flavour.
            let st = (*ep).state();
            (*ep).set_head_state(next, st);
        }
        head
    }
}

/// Remove an arbitrary `tcb` from `ep`'s wait list. Used by
/// `finalize` and by Suspend on a blocked TCB.
pub unsafe fn remove_waiter(ep: *mut Endpoint, tcb: *mut Tcb) {
    if ep.is_null() || tcb.is_null() {
        return;
    }
    unsafe {
        let prev = (*tcb).queue_prev as *mut Tcb;
        let next = (*tcb).queue_next as *mut Tcb;
        if !prev.is_null() {
            (*prev).queue_next = next as u64;
        } else if (*ep).head() == tcb {
            // tcb was head — promote next.
            let st = (*ep).state();
            (*ep).set_head_state(next, st);
        }
        if !next.is_null() {
            (*next).queue_prev = prev as u64;
        } else if (*ep).tail_ptr() == tcb {
            (*ep).set_tail(prev);
        }
        (*tcb).queue_next = 0;
        (*tcb).queue_prev = 0;
        if (*ep).head().is_null() {
            (*ep).set_head_state(core::ptr::null_mut(), EpState::Idle);
            (*ep).set_tail(core::ptr::null_mut());
        }
    }
}

/// Drain `ep`'s wait list on destruction: wake every blocked TCB so
/// the cap-revoke teardown doesn't leak threads. Each woken TCB is
/// marked Restart and pushed back onto the global runqueue.
pub unsafe fn finalize(ep: *mut Endpoint) {
    if ep.is_null() {
        return;
    }
    loop {
        let head = unsafe { pop_head(ep) };
        if head.is_null() {
            break;
        }
        unsafe {
            (*head).waiting_on = 0;
            (*head).state = crate::object::tcb::ThreadState::Restart as u8;
            crate::object::tcb::enqueue(head);
        }
    }
}
