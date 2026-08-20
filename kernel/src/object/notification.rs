//! `notification_t` — async signal/wait object.
//!
//! Layout matches `struct notification` from the C kernel
//! (`build-riscv64/kernel/generated/arch/object/structures_gen.h`) so
//! every Notification we hand back to user-space via a cap can be
//! interpreted by external tooling without translation:
//!
//! ```text
//!   words[0]: state          : bits 0..2
//!             ntfnQueue_tail : bits 25..64 (low 39 bits of ptr, sign-ext on read)
//!   words[1]: ntfnQueue_head : full word, treated as raw kernel ptr
//!   words[2]: ntfnMsgIdentifier (badge)
//!   words[3]: ntfnBoundTCB    : full word, treated as raw kernel ptr
//! ```
//!
//! State machine mirrors `sendSignal` / `receiveSignal` in
//! `kernel/src/object/notification.c`:
//!
//! * `Idle`   → no signal, no waiter. `Wait` enqueues caller → `Waiting`.
//!              `Signal` (no bound waiter) latches badge → `Active`.
//! * `Active` → one un-collected badge. `Wait` collects it → `Idle`.
//!              `Signal` ORs new badge into latched value.
//! * `Waiting` → queue holds blocked receivers. `Signal` pops head,
//!              delivers badge directly to the woken TCB.
//!
//! As with endpoints, the wait queue is the TCB-embedded intrusive list; the
//! head and tail words below are its ends.

#![allow(dead_code)]

use crate::ktypes::list::{self, QueueEnds};
use crate::ktypes::objref::{ObjRef, OptObjRefExt};
use crate::object::endpoint::EndpointRef;
use crate::object::tcb::{BlockedReceive, Tcb, TcbRef};

/// Handle for a notification object.
pub type NotificationRef = ObjRef<Notification>;

#[repr(C)]
#[derive(Copy, Clone, Default)]
pub struct Notification {
    words: [u64; 4],
}

const _: () = {
    assert!(size_of::<Notification>() == 32);
    assert!(align_of::<Notification>() >= 8);
};

#[derive(Copy, Clone, PartialEq, Eq, Debug, Default)]
#[repr(u64)]
pub enum NtfnState {
    #[default]
    Idle = 0,
    Waiting = 1,
    Active = 2,
}

/// Sign-extend a 39-bit value to a full 64-bit kernel pointer.
#[inline]
fn sign_extend_39(v: u64) -> u64 {
    if v & (1u64 << 38) != 0 {
        v | 0xffffff80_00000000
    } else {
        v
    }
}

const TAIL_MASK: u64 = 0xfffffffffe000000;

#[inline]
fn is_kernel_pspace_kva(kva: u64) -> bool {
    kva >= crate::abi::constants::PPTR_BASE as u64 && kva < crate::abi::constants::PPTR_TOP as u64
}

impl Notification {
    pub const fn new() -> Self {
        Notification { words: [0; 4] }
    }

    #[inline]
    fn state(&self) -> NtfnState {
        match self.words[0] & 0x3 {
            1 => NtfnState::Waiting,
            2 => NtfnState::Active,
            _ => NtfnState::Idle,
        }
    }

    #[inline]
    fn set_state(&mut self, s: NtfnState) {
        self.words[0] = (self.words[0] & !0x3u64) | (s as u64);
    }

    /// Badge / message identifier last delivered by a `Signal`.
    #[inline]
    fn badge(&self) -> u64 {
        self.words[2]
    }

    #[inline]
    fn set_badge(&mut self, b: u64) {
        self.words[2] = b;
    }

    /// The bound TCB, if the notification has one.
    #[inline]
    fn bound_tcb(&self) -> Option<TcbRef> {
        // SAFETY: the word only ever holds an address stored from a live
        // `TcbRef`.
        unsafe { ObjRef::from_kva(self.words[3]) }
    }

    #[inline]
    fn set_bound_tcb(&mut self, tcb: Option<TcbRef>) {
        self.words[3] = tcb.kva_or_zero();
    }

    /// Collect a latched badge, moving back to `Idle`.
    fn take_active_badge(&mut self) -> Option<u64> {
        if self.state() != NtfnState::Active {
            return None;
        }
        let badge = self.badge();
        self.set_badge(0);
        self.set_state(NtfnState::Idle);
        Some(badge)
    }
}

impl QueueEnds<Tcb> for Notification {
    #[inline]
    fn head(&self) -> Option<TcbRef> {
        // SAFETY: the word only ever holds an address stored from a live
        // `TcbRef`.
        unsafe { ObjRef::from_kva(self.words[1]) }
    }

    /// Tail is stored as the low 39 bits of the pointer, packed at
    /// bits 25..64 of `words[0]` so it sits next to `state`. Read-back
    /// sign-extends bit 38 to recover the full kernel pointer.
    #[inline]
    fn tail(&self) -> Option<TcbRef> {
        let raw = (self.words[0] & TAIL_MASK) >> 25;
        if raw == 0 {
            return None;
        }
        // SAFETY: as `head`, with the ABI's 39-bit packing undone.
        unsafe { ObjRef::from_kva(sign_extend_39(raw)) }
    }

    #[inline]
    fn set_head(&mut self, head: Option<TcbRef>) {
        self.words[1] = head.kva_or_zero();
    }

    #[inline]
    fn set_tail(&mut self, tail: Option<TcbRef>) {
        let packed = (tail.kva_or_zero() << 25) & TAIL_MASK;
        self.words[0] = (self.words[0] & !TAIL_MASK) | packed;
    }
}

/// Initialise a freshly-retyped Notification slab. `Untyped_Retype`
/// already zeroed the memory; nothing else to do.
///
/// # Safety
/// `ntfn_kva` must be the base of a zeroed slab the caller has just retyped
/// into a Notification object.
pub unsafe fn init(_ntfn_kva: u64) {
    crate::kernel::smp::debug_assert_kernel_lock_held();
}

/// Result of a `receiveSignal` / `Wait` on a Notification.
pub enum WaitOutcome {
    /// Notification already had a pending signal; caller resumes
    /// immediately with this badge.
    Got(u64),
    /// Caller has been blocked on the notification — scheduler will
    /// pick a different runnable TCB on the next `kernel_exit`.
    Blocked,
}

impl NotificationRef {
    #[inline]
    pub fn state(self) -> NtfnState {
        self.with(Notification::state)
    }

    #[inline]
    pub fn bound_tcb(self) -> Option<TcbRef> {
        self.with(Notification::bound_tcb)
    }

    #[inline]
    pub fn set_bound_tcb(self, tcb: Option<TcbRef>) {
        self.with_mut(|n| n.set_bound_tcb(tcb));
    }

    /// A notification can only be bound while it has no waiters and no
    /// existing binding.
    #[inline]
    pub fn can_bind(self) -> bool {
        self.with(|n| QueueEnds::head(n).is_none() && n.bound_tcb().is_none())
    }

    /// Collect a latched badge if one is pending.
    #[inline]
    pub fn consume_active(self) -> Option<u64> {
        self.with_mut(Notification::take_active_badge)
    }

    /// Append `tcb` to the tail of the wait queue. The caller marks the TCB
    /// blocked and takes it off the runqueue first.
    pub fn enqueue_waiter(self, tcb: TcbRef) {
        self.with_mut(|n| list::push_back(n, tcb));
    }

    /// Remove and return the first waiter, leaving its thread state alone.
    pub fn pop_head(self) -> Option<TcbRef> {
        let popped = self.with_mut(list::pop_front);
        self.with_mut(|n| {
            if QueueEnds::head(n).is_none() {
                n.set_tail(None);
            }
        });
        popped
    }

    /// Remove an arbitrary waiter. Used by suspend/finalize on a
    /// notification-blocked TCB.
    pub fn remove_waiter(self, tcb: TcbRef) {
        self.with_mut(|n| {
            if !list::contains(n, tcb) {
                return;
            }
            list::remove(n, tcb);
            if QueueEnds::head(n).is_none() {
                n.set_tail(None);
                n.set_state(NtfnState::Idle);
            }
        });
    }

    pub fn contains_waiter(self, tcb: TcbRef) -> bool {
        self.with(|n| list::contains(n, tcb))
    }

    /// Move `tcb` to the tail of the wait queue, if it really is waiting here.
    pub fn reorder_waiter(self, tcb: TcbRef) {
        if self.state() != NtfnState::Waiting
            || !self.contains_waiter(tcb)
            || !tcb.waits_on_notification(self)
        {
            return;
        }
        self.remove_waiter(tcb);
        self.enqueue_waiter(tcb);
        self.with_mut(|n| n.set_state(NtfnState::Waiting));
    }

    /// Detach the whole wait queue and go Idle, returning the old head.
    fn take_all_waiting(self) -> Option<TcbRef> {
        self.with_mut(|n| {
            if n.state() != NtfnState::Waiting {
                return None;
            }
            let head = QueueEnds::head(n);
            n.set_head(None);
            n.set_tail(None);
            n.set_state(NtfnState::Idle);
            head
        })
    }

    /// `Signal` on a (possibly badged) Notification cap.
    ///
    /// Matches `sendSignal()` in `kernel/src/object/notification.c`:
    ///
    /// * `Idle`    + bound TCB blocked-on-Receive  → cancel its IPC, wake it,
    ///                                              deliver `badge` in a0.
    ///             + (no bound TCB, or bound TCB not waiting) → latch badge,
    ///                                              flip to `Active`.
    /// * `Waiting` → pop head of wait queue, deliver `badge`, mark Running.
    ///              Queue empty afterwards ⇒ state goes Idle.
    /// * `Active`  → OR `badge` into the latched value.
    pub fn signal(self, badge: u64) {
        match self.state() {
            NtfnState::Waiting => self.signal_waiting(badge),
            NtfnState::Idle | NtfnState::Active => self.signal_idle_or_active(badge),
        }
    }

    fn signal_waiting(self, badge: u64) {
        let Some(dest) = self.pop_head() else {
            debug_assert!(false, "Waiting Notification must have non-empty queue");
            self.with_mut(|n| n.set_state(NtfnState::Idle));
            return;
        };
        self.with_mut(|n| {
            if QueueEnds::head(n).is_none() {
                n.set_state(NtfnState::Idle);
            }
        });
        dest.complete_notification_wait(badge);
        dest.enqueue();
    }

    fn signal_idle_or_active(self, badge: u64) {
        let (combined, bound) = self.with(|n| {
            let latched = if n.state() == NtfnState::Active {
                n.badge()
            } else {
                0
            };
            (latched | badge, n.bound_tcb())
        });

        // A bound thread that is blocked receiving takes delivery directly,
        // which for an endpoint receive means cancelling that receive first.
        match bound.and_then(TcbRef::blocked_receive) {
            Some(BlockedReceive::OnEndpoint(ep)) => {
                let bound = bound.expect("blocked_receive implies a bound thread");
                if self.deliver_to_bound_endpoint_receiver(bound, ep, combined) {
                    return;
                }
                // The receive was cancelled underneath us; fall back to
                // latching the badge.
                self.latch(combined);
            }
            Some(BlockedReceive::Detached) => {
                let bound = bound.expect("blocked_receive implies a bound thread");
                self.with_mut(|n| {
                    n.set_badge(0);
                    n.set_state(NtfnState::Idle);
                });
                bound.complete_notification_wait(combined);
                bound.enqueue();
            }
            None => self.latch(combined),
        }
    }

    fn latch(self, badge: u64) {
        self.with_mut(|n| {
            n.set_badge(badge);
            n.set_state(NtfnState::Active);
        });
    }

    /// Take a bound thread off `ep`'s receive queue and hand it `badge`.
    /// Returns false if the thread turned out not to be waiting there.
    fn deliver_to_bound_endpoint_receiver(self, tcb: TcbRef, ep: EndpointRef, badge: u64) -> bool {
        if self.bound_tcb() != Some(tcb) || !tcb.waits_on_endpoint(ep, false) {
            return false;
        }
        ep.remove_waiter(tcb);
        self.with_mut(|n| {
            n.set_badge(0);
            n.set_state(NtfnState::Idle);
        });
        tcb.complete_bound_endpoint_notification_wait(badge);
        tcb.enqueue();
        true
    }

    /// `Wait` on a Notification.
    ///
    /// * `Active`  → collect the latched badge, flip to `Idle`, return Got.
    /// * `Idle/Waiting` + blocking → enqueue caller in wait queue, flip to
    ///                               `Waiting`, return Blocked.
    /// * `Idle/Waiting` + non-blocking → caller wants to poll; return Got(0).
    ///
    /// The caller writes badge into A0 and clears A1..A5 / MR[0..3]; this only
    /// handles queue and state bookkeeping.
    pub fn wait(self, tcb: TcbRef, blocking: bool) -> WaitOutcome {
        if let Some(badge) = self.consume_active() {
            return WaitOutcome::Got(badge);
        }
        if !blocking {
            return WaitOutcome::Got(0);
        }
        tcb.dequeue();
        tcb.set_blocked_on_notification(self);
        self.enqueue_waiter(tcb);
        self.with_mut(|n| n.set_state(NtfnState::Waiting));
        WaitOutcome::Blocked
    }

    /// Cancel-all on Notification destruction: waiting threads move to
    /// `Restart` and re-enter the runqueue, mirroring `cancelAllSignals` in
    /// the C kernel. Active badges are preserved; only Waiting queues are
    /// detached.
    pub fn finalize(self) {
        crate::kernel::smp::debug_assert_kernel_lock_held();
        self.unbind_bound_tcb();
        let mut next = self.take_all_waiting();
        while let Some(waiter) = next {
            next = waiter.restart_notification_waiter();
            waiter.enqueue();
        }
    }

    fn unbind_bound_tcb(self) {
        let bound = self.with_mut(|n| {
            let bound = n.bound_tcb();
            n.set_bound_tcb(None);
            bound
        });
        // The bound thread lives in user-retyped memory, so only follow the
        // link if it still points into the kernel window.
        if let Some(bound) = bound
            && is_kernel_pspace_kva(bound.kva())
        {
            bound.clear_bound_notification_if(self);
        }
    }
}
