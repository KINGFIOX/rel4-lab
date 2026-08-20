//! Per-core mutable globals.
//!
//! Data that is private to one core still lives in a `static`, so Rust asks
//! for `Sync` and for interior mutability. [`PerCpu`] provides both: one slot
//! per core, indexed by the running core's id, handed out as a scoped borrow.
//!
//! Access is serialised the same way the rest of the kernel's shared state is,
//! by the big kernel lock. That is stronger than strictly necessary for
//! core-private data, but it keeps one rule for the whole kernel, and it means
//! cross-core inspection (one core stalling another core's current thread)
//! uses the same type.

use core::cell::UnsafeCell;

use crate::kernel::smp::{MAX_BOOT_CPUS, current_core_id, debug_assert_kernel_lock_held};

/// The value a core's slot starts out holding.
///
/// Needed because a `static PerCpu<T>` has to be built in a const context, and
/// there is no const way to clone a value into every slot.
pub trait PerCpuInit: Sized {
    const INIT: Self;
}

/// One `T` per core.
#[repr(transparent)]
pub struct PerCpu<T> {
    slots: [UnsafeCell<T>; MAX_BOOT_CPUS],
}

// SAFETY: the slots are only reachable through the scoped accessors below,
// which require the big kernel lock, so no two cores touch a slot at once.
unsafe impl<T: Send> Sync for PerCpu<T> {}

impl<T: PerCpuInit> PerCpu<T> {
    pub const fn new() -> Self {
        Self {
            slots: [const { UnsafeCell::new(T::INIT) }; MAX_BOOT_CPUS],
        }
    }
}

impl<T: PerCpuInit> Default for PerCpu<T> {
    fn default() -> Self {
        Self::new()
    }
}

impl<T> PerCpu<T> {
    /// Borrow the running core's slot.
    #[inline]
    pub fn with<R>(&self, op: impl FnOnce(&T) -> R) -> R {
        self.with_core(current_core_id(), op)
            .expect("percpu: current core id out of range")
    }

    /// Borrow the running core's slot mutably.
    #[inline]
    pub fn with_mut<R>(&self, op: impl FnOnce(&mut T) -> R) -> R {
        self.with_core_mut(current_core_id(), op)
            .expect("percpu: current core id out of range")
    }

    /// Borrow `core`'s slot, or `None` if there is no such core.
    #[inline]
    pub fn with_core<R>(&self, core: usize, op: impl FnOnce(&T) -> R) -> Option<R> {
        debug_assert_kernel_lock_held();
        let slot = self.slots.get(core)?;
        // SAFETY: the big kernel lock serialises access, and the borrow ends
        // with `op` so it cannot overlap another borrow of the same slot.
        Some(op(unsafe { &*slot.get() }))
    }

    /// Borrow `core`'s slot mutably, or `None` if there is no such core.
    #[inline]
    pub fn with_core_mut<R>(&self, core: usize, op: impl FnOnce(&mut T) -> R) -> Option<R> {
        debug_assert_kernel_lock_held();
        let slot = self.slots.get(core)?;
        // SAFETY: as `with_core`; the exclusive borrow lasts only for `op`.
        Some(op(unsafe { &mut *slot.get() }))
    }

    /// Borrow the running core's slot without taking the big kernel lock.
    ///
    /// For state that is private to one core and never inspected by another:
    /// the hardware descriptor tables a core installs during its own boot.
    ///
    /// # Safety
    /// No other borrow of this core's slot may be live, and no other core may
    /// access this slot.
    #[inline]
    pub unsafe fn with_core_private_mut<R>(&self, op: impl FnOnce(&mut T) -> R) -> R {
        let slot = self
            .slots
            .get(current_core_id())
            .expect("percpu: current core id out of range");
        // SAFETY: the caller promised this slot is core-private and not
        // otherwise borrowed.
        op(unsafe { &mut *slot.get() })
    }

    /// Address of `core`'s slot, for the cases where hardware or assembly
    /// needs the address of per-core state rather than its value.
    #[inline]
    pub fn slot_ptr(&self, core: usize) -> Option<*mut T> {
        Some(self.slots.get(core)?.get())
    }
}
