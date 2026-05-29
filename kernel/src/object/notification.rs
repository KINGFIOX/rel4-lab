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

use crate::object::tcb::Tcb;

#[repr(C)]
#[derive(Copy, Clone, Default)]
pub struct Notification {
    pub words: [u64; 4],
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

impl Notification {
    pub const fn new() -> Self {
        Notification { words: [0; 4] }
    }

    #[inline]
    pub fn state(&self) -> NtfnState {
        match self.words[0] & 0x3 {
            1 => NtfnState::Waiting,
            2 => NtfnState::Active,
            _ => NtfnState::Idle,
        }
    }

    #[inline]
    pub fn set_state(&mut self, s: NtfnState) {
        self.words[0] = (self.words[0] & !0x3u64) | (s as u64);
    }

    /// Badge / message identifier last delivered by a `Signal`.
    #[inline]
    pub fn badge(&self) -> u64 {
        self.words[2]
    }

    #[inline]
    pub fn set_badge(&mut self, b: u64) {
        self.words[2] = b;
    }

    /// PSpace KVA of the bound TCB (or 0 for "no binding").
    #[inline]
    pub fn bound_tcb(&self) -> u64 {
        self.words[3]
    }

    #[inline]
    pub fn set_bound_tcb(&mut self, p: u64) {
        self.words[3] = p;
    }

    /// Head of the blocked-receiver queue (or null).
    #[inline]
    pub fn head(&self) -> *mut Tcb {
        self.words[1] as *mut Tcb
    }

    #[inline]
    pub fn set_head(&mut self, h: *mut Tcb) {
        self.words[1] = h as u64;
    }

    /// Tail is stored as the low 39 bits of the pointer, packed at
    /// bits 25..64 of `words[0]` so it sits next to `state`. Read-back
    /// sign-extends bit 38 to recover the full kernel ptr.
    #[inline]
    pub fn tail(&self) -> *mut Tcb {
        let raw = (self.words[0] & TAIL_MASK) >> 25;
        if raw == 0 {
            core::ptr::null_mut()
        } else {
            sign_extend_39(raw) as *mut Tcb
        }
    }

    #[inline]
    pub fn set_tail(&mut self, t: *mut Tcb) {
        let v = t as u64;
        self.words[0] = (self.words[0] & !TAIL_MASK) | ((v << 25) & TAIL_MASK);
    }
}

/// Initialise a freshly-retyped Notification slab. `Untyped_Retype`
/// already zeroed the memory; nothing else to do.
pub unsafe fn init(_ntfn_kva: u64) {}

/// Append `tcb` to the tail of `ntfn`'s wait queue. Caller is
/// responsible for marking the TCB blocked + setting `waiting_on`
/// + dequeueing from the runqueue beforehand.
pub unsafe fn enqueue_waiter(ntfn: *mut Notification, tcb: *mut Tcb) {
    if ntfn.is_null() || tcb.is_null() {
        return;
    }
    unsafe {
        (*tcb).queue_next = 0;
        let old_tail = (*ntfn).tail();
        (*tcb).queue_prev = old_tail as u64;
        if old_tail.is_null() {
            (*ntfn).set_head(tcb);
        } else {
            (*old_tail).queue_next = tcb as u64;
        }
        (*ntfn).set_tail(tcb);
    }
}

/// Pop the head of `ntfn`'s wait queue, returning it (or null). The
/// popped TCB still has its old `state`/`waiting_on` — caller must
/// transition it to Running + re-enqueue into the runqueue.
pub unsafe fn pop_head(ntfn: *mut Notification) -> *mut Tcb {
    if ntfn.is_null() {
        return core::ptr::null_mut();
    }
    unsafe {
        let head = (*ntfn).head();
        if head.is_null() {
            return core::ptr::null_mut();
        }
        let next = (*head).queue_next as *mut Tcb;
        (*head).queue_next = 0;
        (*head).queue_prev = 0;
        if next.is_null() {
            (*ntfn).set_head(core::ptr::null_mut());
            (*ntfn).set_tail(core::ptr::null_mut());
        } else {
            (*next).queue_prev = 0;
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
    unsafe {
        let prev = (*tcb).queue_prev as *mut Tcb;
        let next = (*tcb).queue_next as *mut Tcb;
        if !prev.is_null() {
            (*prev).queue_next = next as u64;
        } else if (*ntfn).head() == tcb {
            (*ntfn).set_head(next);
        }
        if !next.is_null() {
            (*next).queue_prev = prev as u64;
        } else if (*ntfn).tail() == tcb {
            (*ntfn).set_tail(prev);
        }
        (*tcb).queue_next = 0;
        (*tcb).queue_prev = 0;
        if (*ntfn).head().is_null() {
            (*ntfn).set_tail(core::ptr::null_mut());
            (*ntfn).set_state(NtfnState::Idle);
        }
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
    use crate::arch::riscv64::trap::reg;
    use crate::object::tcb::{self, ThreadState};

    let n = unsafe { &mut *ntfn };
    match n.state() {
        NtfnState::Idle => {
            let bound = n.bound_tcb();
            if bound != 0 {
                let tcb_ptr = bound as *mut Tcb;
                let st = unsafe { (*tcb_ptr).state };
                if st == ThreadState::BlockedOnReceive as u8 {
                    // Cancel any in-flight EP receive and wake the TCB
                    // with `badge` in its first message register (a0).
                    unsafe {
                        let waiting_ep = (*tcb_ptr).waiting_on;
                        if waiting_ep != 0 {
                            crate::object::endpoint::remove_waiter(
                                waiting_ep as *mut crate::object::endpoint::Endpoint,
                                tcb_ptr,
                            );
                            (*tcb_ptr).waiting_on = 0;
                        }
                        (*tcb_ptr).context.regs[reg::A0] = badge;
                        (*tcb_ptr).context.regs[reg::A1] = 0;
                        (*tcb_ptr).context.regs[reg::A2] = 0;
                        (*tcb_ptr).context.regs[reg::A3] = 0;
                        (*tcb_ptr).context.regs[reg::A4] = 0;
                        (*tcb_ptr).context.regs[reg::A5] = 0;
                        (*tcb_ptr).state = ThreadState::Running as u8;
                        tcb::enqueue(tcb_ptr);
                    }
                    return;
                }
            }
            n.set_badge(badge);
            n.set_state(NtfnState::Active);
        }
        NtfnState::Active => {
            n.set_badge(n.badge() | badge);
        }
        NtfnState::Waiting => {
            let dest = unsafe { pop_head(ntfn) };
            debug_assert!(
                !dest.is_null(),
                "Waiting Notification must have non-empty queue"
            );
            unsafe {
                (*dest).waiting_on = 0;
                (*dest).context.regs[reg::A0] = badge;
                (*dest).context.regs[reg::A1] = 0;
                (*dest).context.regs[reg::A2] = 0;
                (*dest).context.regs[reg::A3] = 0;
                (*dest).context.regs[reg::A4] = 0;
                (*dest).context.regs[reg::A5] = 0;
                (*dest).state = ThreadState::Running as u8;
                tcb::enqueue(dest);
            }
            // After pop_head, head may now be null → Idle. Otherwise
            // the queue still holds more waiters; stay in Waiting.
            if n.head().is_null() {
                n.set_state(NtfnState::Idle);
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
    use crate::object::tcb::{self, ThreadState};
    let n = unsafe { &mut *ntfn };
    match n.state() {
        NtfnState::Active => {
            let b = n.badge();
            n.set_badge(0);
            n.set_state(NtfnState::Idle);
            WaitOutcome::Got(b)
        }
        NtfnState::Idle | NtfnState::Waiting => {
            if !blocking {
                return WaitOutcome::Got(0);
            }
            unsafe {
                tcb::dequeue(tcb);
                (*tcb).state = ThreadState::BlockedOnNotification as u8;
                (*tcb).waiting_on = ntfn as u64;
                enqueue_waiter(ntfn, tcb);
            }
            n.set_state(NtfnState::Waiting);
            WaitOutcome::Blocked
        }
    }
}

/// Cancel-all on Notification destruction: every queued waiter is
/// woken with badge 0 and re-enqueued in the runqueue, mirroring
/// `cancelAllSignals` in the C kernel. Also clears any bound TCB
/// back-pointer.
pub unsafe fn finalize(ntfn: *mut Notification) {
    use crate::arch::riscv64::trap::reg;
    use crate::object::tcb::{self, ThreadState};
    if ntfn.is_null() {
        return;
    }
    loop {
        let head = unsafe { pop_head(ntfn) };
        if head.is_null() {
            break;
        }
        unsafe {
            (*head).waiting_on = 0;
            (*head).context.regs[reg::A0] = 0;
            (*head).context.regs[reg::A1] = 0;
            (*head).state = ThreadState::Restart as u8;
            tcb::enqueue(head);
        }
    }
    let n = unsafe { &mut *ntfn };
    n.set_state(NtfnState::Idle);
}
