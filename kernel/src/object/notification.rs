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
}

/// `Signal` on a (possibly badged) Notification cap. With no waiter queue
/// implemented yet, this is the trivial "OR the badge into the latched
/// value and mark active" path used by `sendSignal`'s `NtfnState_Idle` /
/// `NtfnState_Active` branches in `kernel/src/object/notification.c`.
pub unsafe fn signal(ntfn: *mut Notification, badge: u64) {
    let n = unsafe { &mut *ntfn };
    match n.state() {
        NtfnState::Idle => {
            n.set_badge(badge);
            n.set_state(NtfnState::Active);
        }
        NtfnState::Active => {
            // Coalesce: kernel does `current_badge | badge`.
            n.set_badge(n.badge() | badge);
        }
        NtfnState::Waiting => {
            // Until we have a TCB queue we cannot have a Waiting state —
            // every Wait that finds Idle would have to block (and we
            // don't support blocking yet). Treat this as a programming
            // error.
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
