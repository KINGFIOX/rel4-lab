//! Marker guards for fixed-layout wait-queue objects under the BKL.
//!
//! Endpoint and Notification object mutation is serialized by the
//! seL4-style big kernel lock, not by per-object locks.

use crate::kernel::smp::BklObjectGuard;

pub type WaitQueueLockGuard = BklObjectGuard;

pub struct WaitQueueLockPair {
    _first: WaitQueueLockGuard,
    _second: Option<WaitQueueLockGuard>,
}

#[inline]
pub fn lock(_object: *const ()) -> WaitQueueLockGuard {
    BklObjectGuard::new()
}

pub fn lock_pair(_a: *const (), _b: *const ()) -> WaitQueueLockPair {
    WaitQueueLockPair {
        _first: BklObjectGuard::new(),
        _second: None,
    }
}
