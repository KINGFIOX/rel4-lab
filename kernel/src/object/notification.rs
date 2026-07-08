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

#![allow(dead_code)]

use crate::object::endpoint;
use crate::object::tcb::{self, Tcb};
use crate::object::wait_queue_lock::{self, WaitQueueLockGuard};

#[repr(C)]
#[derive(Copy, Clone, Default)]
pub struct Notification {
    words: [u64; 4],
}

const _: () = {
    assert!(core::mem::size_of::<Notification>() == 32);
    assert!(core::mem::align_of::<Notification>() >= 8);
};

#[derive(Copy, Clone, PartialEq, Eq, Debug)]
#[repr(u64)]
pub enum NtfnState {
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

    /// PSpace KVA of the bound TCB (or 0 for "no binding").
    #[inline]
    fn bound_tcb(&self) -> u64 {
        self.words[3]
    }

    #[inline]
    pub(crate) fn set_bound_tcb(&mut self, p: u64) {
        self.words[3] = p;
    }

    /// Head of the blocked-receiver queue (or null).
    #[inline]
    fn head(&self) -> *mut Tcb {
        self.words[1] as *mut Tcb
    }

    #[inline]
    fn set_head(&mut self, h: *mut Tcb) {
        self.words[1] = h as u64;
    }

    /// Tail is stored as the low 39 bits of the pointer, packed at
    /// bits 25..64 of `words[0]` so it sits next to `state`. Read-back
    /// sign-extends bit 38 to recover the full kernel ptr.
    #[inline]
    fn tail(&self) -> *mut Tcb {
        let raw = (self.words[0] & TAIL_MASK) >> 25;
        if raw == 0 {
            core::ptr::null_mut()
        } else {
            sign_extend_39(raw) as *mut Tcb
        }
    }

    #[inline]
    fn set_tail(&mut self, t: *mut Tcb) {
        let v = t as u64;
        self.words[0] = (self.words[0] & !TAIL_MASK) | ((v << 25) & TAIL_MASK);
    }
}

/// Initialise a freshly-retyped Notification slab. `Untyped_Retype`
/// already zeroed the memory; nothing else to do.
pub unsafe fn init(_ntfn_kva: u64) {
    crate::kernel::smp::debug_assert_kernel_lock_held();
}

#[inline]
pub(crate) unsafe fn lock_queue(ntfn: *const Notification) -> WaitQueueLockGuard {
    wait_queue_lock::lock(ntfn.cast())
}

pub(crate) unsafe fn bound_tcb_snapshot(ntfn: *const Notification) -> *mut Tcb {
    if ntfn.is_null() {
        return core::ptr::null_mut();
    }
    let _guard = unsafe { lock_queue(ntfn) };
    unsafe { (*ntfn).bound_tcb() as *mut Tcb }
}

pub(crate) unsafe fn can_bind_snapshot(ntfn: *const Notification) -> bool {
    if ntfn.is_null() {
        return false;
    }
    let _guard = unsafe { lock_queue(ntfn) };
    unsafe { (*ntfn).head().is_null() && (*ntfn).bound_tcb() == 0 }
}

pub unsafe fn consume_active(ntfn: *mut Notification, tcb: *mut Tcb) -> Option<u64> {
    if ntfn.is_null() || tcb.is_null() {
        return None;
    }
    let _guard = unsafe { lock_queue(ntfn) };
    unsafe { consume_active_locked(ntfn, tcb) }
}

/// Caller must hold this Notification's queue lock, or an ordered pair lock
/// that includes it.
pub(crate) unsafe fn consume_active_locked(ntfn: *mut Notification, tcb: *mut Tcb) -> Option<u64> {
    if ntfn.is_null() || tcb.is_null() {
        return None;
    }
    unsafe {
        let n = &mut *ntfn;
        if n.state() != NtfnState::Active {
            return None;
        }
        let badge = n.badge();
        n.set_badge(0);
        n.set_state(NtfnState::Idle);
        Some(badge)
    }
}

/// Append `tcb` to the tail of `ntfn`'s wait queue. Caller is
/// responsible for marking the TCB blocked + setting `waiting_on`
/// + dequeueing from the runqueue beforehand.
pub unsafe fn enqueue_waiter(ntfn: *mut Notification, tcb: *mut Tcb) {
    if ntfn.is_null() || tcb.is_null() {
        return;
    }
    let _guard = unsafe { lock_queue(ntfn) };
    unsafe {
        enqueue_waiter_locked(ntfn, tcb);
    }
}

pub(crate) unsafe fn enqueue_waiter_locked(ntfn: *mut Notification, tcb: *mut Tcb) {
    if ntfn.is_null() || tcb.is_null() {
        return;
    }
    unsafe {
        let tail = (*ntfn).tail();
        if tail.is_null() {
            (*ntfn).set_head(tcb);
        } else {
            tcb::set_wait_queue_next(tail, tcb);
        }

        (*ntfn).set_tail(tcb);
        tcb::set_wait_queue_links(tcb, tail, core::ptr::null_mut());
    }
}

/// Pop the head of `ntfn`'s wait queue, returning it (or null). The
/// popped TCB still has its old `state`/`waiting_on` — caller must
/// transition it to Running + re-enqueue into the runqueue.
pub unsafe fn pop_head(ntfn: *mut Notification) -> *mut Tcb {
    if ntfn.is_null() {
        return core::ptr::null_mut();
    }
    let _guard = unsafe { lock_queue(ntfn) };
    unsafe { pop_head_locked(ntfn) }
}

pub(crate) unsafe fn pop_head_locked(ntfn: *mut Notification) -> *mut Tcb {
    if ntfn.is_null() {
        return core::ptr::null_mut();
    }
    unsafe {
        let head = (*ntfn).head();
        if head.is_null() {
            return core::ptr::null_mut();
        }
        let next = tcb::wait_queue_next_snapshot(head);
        tcb::clear_wait_queue_links(head);
        if next.is_null() {
            (*ntfn).set_head(core::ptr::null_mut());
            (*ntfn).set_tail(core::ptr::null_mut());
        } else {
            tcb::set_wait_queue_prev(next, core::ptr::null_mut());
            (*ntfn).set_head(next);
        }
        head
    }
}

/// Remove an arbitrary `tcb` from `ntfn`'s wait queue. Used by
/// suspend/finalize on a notification-blocked TCB.
pub unsafe fn remove_waiter(ntfn: *mut Notification, tcb: *mut Tcb) {
    if ntfn.is_null() || tcb.is_null() {
        return;
    }
    let _guard = unsafe { lock_queue(ntfn) };
    unsafe {
        if !contains_waiter_locked(ntfn, tcb) {
            return;
        }
        remove_waiter_locked(ntfn, tcb);
    }
}

pub(crate) unsafe fn remove_waiter_locked(ntfn: *mut Notification, tcb: *mut Tcb) {
    if ntfn.is_null() || tcb.is_null() {
        return;
    }
    unsafe {
        let (prev, next) = tcb::wait_queue_links_snapshot(tcb);
        if !prev.is_null() {
            tcb::set_wait_queue_next(prev, next);
        } else if (*ntfn).head() == tcb {
            (*ntfn).set_head(next);
        }
        if !next.is_null() {
            tcb::set_wait_queue_prev(next, prev);
        } else if (*ntfn).tail() == tcb {
            (*ntfn).set_tail(prev);
        }
        tcb::clear_wait_queue_links(tcb);
        if (*ntfn).head().is_null() {
            (*ntfn).set_tail(core::ptr::null_mut());
            (*ntfn).set_state(NtfnState::Idle);
        }
    }
}

unsafe fn contains_waiter_locked(ntfn: *mut Notification, tcb: *mut Tcb) -> bool {
    if ntfn.is_null() || tcb.is_null() {
        return false;
    }
    unsafe {
        let mut cur = (*ntfn).head();
        while !cur.is_null() {
            if cur == tcb {
                return true;
            }
            cur = tcb::wait_queue_next_snapshot(cur);
        }
    }
    false
}

unsafe fn waiter_matches_notification_locked(ntfn: *mut Notification, tcb: *mut Tcb) -> bool {
    if unsafe { (*ntfn).state() != NtfnState::Waiting || !contains_waiter_locked(ntfn, tcb) } {
        return false;
    }
    tcb::blocked_on_notification_for(tcb, ntfn as u64)
}

pub unsafe fn reorder_waiter(ntfn: *mut Notification, tcb: *mut Tcb) {
    if ntfn.is_null() || tcb.is_null() {
        return;
    }
    let _guard = unsafe { lock_queue(ntfn) };
    unsafe {
        if !waiter_matches_notification_locked(ntfn, tcb) {
            return;
        }
        remove_waiter_locked(ntfn, tcb);
        enqueue_waiter_locked(ntfn, tcb);
        (*ntfn).set_state(NtfnState::Waiting);
    }
}

unsafe fn take_all_locked(ntfn: *mut Notification) -> *mut Tcb {
    unsafe {
        if (*ntfn).state() != NtfnState::Waiting {
            return core::ptr::null_mut();
        }
        let head = (*ntfn).head();
        (*ntfn).set_head(core::ptr::null_mut());
        (*ntfn).set_tail(core::ptr::null_mut());
        (*ntfn).set_state(NtfnState::Idle);
        head
    }
}

struct DetachedNotification {
    waiters: *mut Tcb,
}

unsafe fn unbind_bound_tcb_locked(ntfn: *mut Notification) {
    unsafe {
        let bound_tcb = (*ntfn).bound_tcb();
        if bound_tcb == 0 {
            return;
        }
        (*ntfn).set_bound_tcb(0);
        if is_kernel_pspace_kva(bound_tcb) {
            tcb::clear_bound_notification_if(bound_tcb as *mut Tcb, ntfn as u64);
        }
    }
}

unsafe fn detach_final_state_locked(ntfn: *mut Notification) -> DetachedNotification {
    unsafe {
        unbind_bound_tcb_locked(ntfn);
        let waiters = take_all_locked(ntfn);
        DetachedNotification { waiters }
    }
}

unsafe fn prepare_signal_receiver_locked(_ntfn: *mut Notification, tcb: *mut Tcb, badge: u64) {
    unsafe {
        tcb::complete_notification_wait(tcb, badge);
    }
}

unsafe fn try_signal_bound_endpoint(
    ntfn: *mut Notification,
    badge: u64,
    tcb: *mut Tcb,
    ep: *mut endpoint::Endpoint,
) -> Option<(*mut Tcb, u64)> {
    if ntfn.is_null() || tcb.is_null() || ep.is_null() {
        return None;
    }
    let _guard = wait_queue_lock::lock_pair(ntfn.cast(), ep.cast());
    unsafe {
        let n = &mut *ntfn;
        let combined = match n.state() {
            NtfnState::Idle => badge,
            NtfnState::Active => n.badge() | badge,
            NtfnState::Waiting => return None,
        };
        if n.bound_tcb() != tcb as u64 {
            return None;
        }
        if !tcb::waiter_matches_endpoint(tcb, ep as u64, false) {
            return None;
        }
        endpoint::remove_waiter_locked(ep, tcb);
        n.set_badge(0);
        n.set_state(NtfnState::Idle);
        Some((tcb, combined))
    }
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
pub unsafe fn signal(ntfn: *mut Notification, badge: u64) {
    enum SignalAction {
        Done,
        Wake(*mut Tcb),
        BoundEndpoint {
            tcb: *mut Tcb,
            ep: *mut endpoint::Endpoint,
        },
    }

    if ntfn.is_null() {
        return;
    }

    loop {
        let action = {
            let _guard = unsafe { lock_queue(ntfn) };
            let n = unsafe { &mut *ntfn };
            match n.state() {
                NtfnState::Idle | NtfnState::Active => {
                    let active_badge = if n.state() == NtfnState::Active {
                        n.badge()
                    } else {
                        0
                    };
                    let combined = active_badge | badge;
                    let bound = n.bound_tcb();
                    if bound != 0 {
                        let tcb_ptr = bound as *mut Tcb;
                        if let Some(waiting_on) = tcb::blocked_receive_endpoint_snapshot(tcb_ptr) {
                            if waiting_on != 0 {
                                SignalAction::BoundEndpoint {
                                    tcb: tcb_ptr,
                                    ep: waiting_on as *mut endpoint::Endpoint,
                                }
                            } else {
                                n.set_badge(0);
                                n.set_state(NtfnState::Idle);
                                unsafe {
                                    prepare_signal_receiver_locked(ntfn, tcb_ptr, combined);
                                }
                                SignalAction::Wake(tcb_ptr)
                            }
                        } else {
                            n.set_badge(combined);
                            n.set_state(NtfnState::Active);
                            SignalAction::Done
                        }
                    } else {
                        n.set_badge(combined);
                        n.set_state(NtfnState::Active);
                        SignalAction::Done
                    }
                }
                NtfnState::Waiting => {
                    let dest = unsafe { pop_head_locked(ntfn) };
                    debug_assert!(
                        !dest.is_null(),
                        "Waiting Notification must have non-empty queue"
                    );
                    if dest.is_null() {
                        n.set_state(NtfnState::Idle);
                        SignalAction::Done
                    } else {
                        if n.head().is_null() {
                            n.set_state(NtfnState::Idle);
                        }
                        unsafe {
                            prepare_signal_receiver_locked(ntfn, dest, badge);
                        }
                        SignalAction::Wake(dest)
                    }
                }
            }
        };

        match action {
            SignalAction::Done => return,
            SignalAction::Wake(tcb_ptr) => {
                unsafe {
                    tcb::enqueue(tcb_ptr);
                }
                return;
            }
            SignalAction::BoundEndpoint { tcb, ep } => {
                if let Some((tcb_ptr, badge)) =
                    unsafe { try_signal_bound_endpoint(ntfn, badge, tcb, ep) }
                {
                    unsafe {
                        tcb::complete_bound_endpoint_notification_wait(tcb_ptr, badge);
                        tcb::enqueue(tcb_ptr);
                    }
                    return;
                }
            }
        }
    }
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

/// `Wait` on a Notification.
///
/// * `Active`  → collect the latched badge, flip to `Idle`, return Got.
/// * `Idle/Waiting` + blocking → enqueue caller in wait queue, flip to
///                               `Waiting`, return Blocked.
/// * `Idle/Waiting` + non-blocking → caller wants to poll; return Got(0).
///
/// The caller writes badge into A0 and clears A1..A5 / MR[0..3]; we
/// only handle queue & state bookkeeping here.
pub unsafe fn wait(ntfn: *mut Notification, tcb: *mut Tcb, blocking: bool) -> WaitOutcome {
    if ntfn.is_null() || tcb.is_null() {
        return WaitOutcome::Got(0);
    }
    let _guard = unsafe { lock_queue(ntfn) };
    let n = unsafe { &mut *ntfn };
    match n.state() {
        NtfnState::Active => {
            let b = unsafe { consume_active_locked(ntfn, tcb) }.unwrap_or(0);
            WaitOutcome::Got(b)
        }
        NtfnState::Idle | NtfnState::Waiting => {
            if !blocking {
                return WaitOutcome::Got(0);
            }
            unsafe {
                tcb::dequeue(tcb);
                tcb::set_blocked_on_notification(tcb, ntfn as u64);
                enqueue_waiter_locked(ntfn, tcb);
            }
            n.set_state(NtfnState::Waiting);
            WaitOutcome::Blocked
        }
    }
}

/// Cancel-all on Notification destruction: waiting threads move to `Restart`
/// and re-enter the runqueue, mirroring `cancelAllSignals` in the C kernel.
/// Active badges are preserved; only Waiting queues are detached.
pub unsafe fn finalize(ntfn: *mut Notification) {
    crate::kernel::smp::debug_assert_kernel_lock_held();
    if ntfn.is_null() {
        return;
    }
    let detached = {
        let _guard = unsafe { lock_queue(ntfn) };
        unsafe { detach_final_state_locked(ntfn) }
    };
    let mut cur = detached.waiters;
    while !cur.is_null() {
        unsafe {
            let next = tcb::restart_notification_waiter(cur);
            tcb::enqueue(cur);
            cur = next;
        }
    }
}
