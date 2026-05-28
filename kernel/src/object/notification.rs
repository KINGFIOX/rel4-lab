//! `notification_t` — async signal/wait object.
//!
//! Layout matches `struct notification` from the C kernel
//! (`build-riscv64/kernel/generated/arch/object/structures_gen.h`), so
//! every Notification we hand back to user-space via a cap can be
//! interpreted by external tooling without translation:
//!
//! ```text
//!   words[0]: state          : bits 0..2
//!             ntfnQueue_tail : bits 25..64 (sign-extended kernel ptr)
//!   words[1]: ntfnQueue_head : bits 0..39  (sign-extended kernel ptr)
//!   words[2]: ntfnMsgIdentifier (badge)
//!   words[3]: ntfnBoundTCB    : bits 0..39  (sign-extended kernel ptr)
//! ```
//!
//! Until we have TCBs and a scheduler, the queue / bound-TCB fields are
//! unused — only `state` and `ntfnMsgIdentifier` are read/written.
//! That's enough for the single-threaded SYSCALL00{10..13} tests, which
//! all do `Signal(ntfn)` immediately followed by `Wait(ntfn)`.

#![allow(dead_code)]

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
    /// Matches the C kernel's `ntfnBoundTCB` field at `words[3]`.
    #[inline]
    pub fn bound_tcb(&self) -> u64 {
        self.words[3]
    }

    #[inline]
    pub fn set_bound_tcb(&mut self, p: u64) {
        self.words[3] = p;
    }
}

/// `Signal` on a (possibly badged) Notification cap.
///
/// Matches the C kernel's `sendSignal` (`kernel/src/object/notification.c`):
///   * `Idle` + bound TCB blocked on Receive  → cancel its IPC,
///     wake it, deliver `badge` straight into its badge register.
///   * `Idle` (no bound TCB, or bound TCB not waiting) → latch
///     `badge` and flip state to `Active`.
///   * `Active`  → OR `badge` into the latched value.
///   * `Waiting` (would-be ntfnQueue head) → handled the same as
///     bound-TCB delivery once we add a Notification wait list; for
///     now treat as `Active` since `wait()` never blocks.
pub unsafe fn signal(ntfn: *mut Notification, badge: u64) {
    use crate::object::tcb::{self, Tcb, ThreadState};

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
                    // The receiver's syscall path will see a0 = badge
                    // and treat this as a Notification delivery.
                    unsafe {
                        let waiting_ep = (*tcb_ptr).waiting_on;
                        if waiting_ep != 0 {
                            crate::object::endpoint::remove_waiter(
                                waiting_ep as *mut crate::object::endpoint::Endpoint,
                                tcb_ptr,
                            );
                            (*tcb_ptr).waiting_on = 0;
                        }
                        use crate::arch::riscv64::trap::reg;
                        (*tcb_ptr).context.regs[reg::A0] = badge;
                        (*tcb_ptr).context.regs[reg::A1] = 0;
                        // Clear MR[0..3] (a2..a5) so a `seL4_GetMR(i)`
                        // from user space sees zeros instead of a
                        // stale register snapshot from the trap.
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
            // No Notification wait list implemented yet — treat as Active.
            n.set_badge(badge);
            n.set_state(NtfnState::Active);
        }
    }
}

/// Result of a `receiveSignal` / `Wait` on a Notification.
pub enum WaitOutcome {
    /// Notification already had a pending signal; caller resumes
    /// immediately with this badge.
    Got(u64),
    /// No pending signal — the caller would block. Single-thread mode
    /// returns this and lets the syscall handler decide whether to
    /// (a) treat as a non-blocking poll that yields 0 or (b) park the
    /// thread until M3.6 scheduling lands.
    WouldBlock,
}

pub unsafe fn wait(ntfn: *mut Notification) -> WaitOutcome {
    let n = unsafe { &mut *ntfn };
    match n.state() {
        NtfnState::Active => {
            let b = n.badge();
            n.set_badge(0);
            n.set_state(NtfnState::Idle);
            WaitOutcome::Got(b)
        }
        _ => WaitOutcome::WouldBlock,
    }
}
