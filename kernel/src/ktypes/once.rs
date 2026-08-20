//! Globals that are filled in once during boot and read afterwards.
//!
//! Descriptor tables and interrupt routing tables are written while a single
//! core is still bringing the machine up, then only read (often by hardware,
//! from a base address the kernel handed over). That is what `static mut` used
//! to express, at the cost of every access being `unsafe`. [`BootOnce`] keeps
//! the same shape but states the rule: exactly one initialisation, then shared
//! reads.

use core::cell::UnsafeCell;
use core::hint;
use core::sync::atomic::{AtomicU8, Ordering};

use crate::ktypes::addr::Kva;

const UNINITIALISED: u8 = 0;
const INITIALISING: u8 = 1;
const READY: u8 = 2;

/// A value initialised once at boot, then read-only.
pub struct BootOnce<T> {
    value: UnsafeCell<T>,
    state: AtomicU8,
}

// SAFETY: after initialisation the value is only ever handed out as `&T`, and
// initialisation itself is guarded by the state word so only one caller can
// ever hold the `&mut T`.
unsafe impl<T: Send + Sync> Sync for BootOnce<T> {}

impl<T> BootOnce<T> {
    /// Create the cell holding its pre-initialisation value, typically all
    /// zeroes.
    pub const fn new(value: T) -> Self {
        Self {
            value: UnsafeCell::new(value),
            state: AtomicU8::new(UNINITIALISED),
        }
    }

    /// Fill in the value. Panics if called twice.
    pub fn init(&self, op: impl FnOnce(&mut T)) {
        assert!(self.claim_for_init(), "BootOnce: initialised twice");
        self.run_init(op);
    }

    /// Fill in the value if this is the first caller, then borrow it. Cores
    /// that arrive later share whatever the first one wrote.
    pub fn get_or_init(&self, op: impl FnOnce(&mut T)) -> &T {
        if self.claim_for_init() {
            self.run_init(op);
        }
        self.get()
    }

    /// Try to become the caller that initialises the value.
    fn claim_for_init(&self) -> bool {
        self.state
            .compare_exchange(
                UNINITIALISED,
                INITIALISING,
                Ordering::AcqRel,
                Ordering::Acquire,
            )
            .is_ok()
    }

    fn run_init(&self, op: impl FnOnce(&mut T)) {
        // SAFETY: `claim_for_init` succeeded, so this is the only caller that
        // ever reaches here, and no `&T` can have been handed out yet because
        // `get` waits until the state reaches `READY`.
        op(unsafe { &mut *self.value.get() });
        self.state.store(READY, Ordering::Release);
    }

    /// Borrow the initialised value, waiting if another core is mid-way
    /// through initialising it. Panics if nobody has started.
    #[inline]
    pub fn get(&self) -> &T {
        loop {
            match self.state.load(Ordering::Acquire) {
                READY => break,
                INITIALISING => hint::spin_loop(),
                _ => panic!("BootOnce: read before initialisation"),
            }
        }
        // SAFETY: initialisation has completed and no mutable borrow can
        // exist afterwards, so shared borrows are fine.
        unsafe { &*self.value.get() }
    }

    #[inline]
    pub fn is_initialised(&self) -> bool {
        self.state.load(Ordering::Acquire) == READY
    }

    /// Address of the value, for handing to hardware such as `lgdt`/`lidt`.
    #[inline]
    pub fn kva(&self) -> Kva {
        Kva::new(self.value.get() as usize)
    }
}
