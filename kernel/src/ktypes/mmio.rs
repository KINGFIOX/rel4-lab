//! Memory-mapped device registers.
//!
//! Device access is inherently a side effect on something outside the
//! program's memory, so the compiler must not reorder, merge, or elide it. The
//! obligation is entirely about *which address* is a device register; once a
//! region has been identified as one, reading and writing it is ordinary work.
//! So [`MmioRegion`] is unsafe to construct and safe to use, and drivers hold
//! one instead of open-coding `read_volatile` on a cast integer.

use core::marker::PhantomData;

use crate::ktypes::addr::Kva;

/// A device register window reachable through the kernel's MMIO mapping.
#[derive(Copy, Clone, Debug)]
pub struct MmioRegion {
    base: Kva,
    len: usize,
}

impl MmioRegion {
    /// Describe a `len`-byte device window mapped at `base`.
    ///
    /// # Safety
    /// `base` must be a kernel-window address that maps `len` bytes of device
    /// registers, mapped non-cacheable as the platform requires, and no other
    /// code may hold a conflicting Rust reference to that memory.
    #[inline]
    pub const unsafe fn new(base: Kva, len: usize) -> Self {
        Self { base, len }
    }

    #[inline]
    pub const fn base(self) -> Kva {
        self.base
    }

    /// Register of type `T` at `offset` bytes into the window.
    ///
    /// Panics if the register would fall outside the window or is misaligned,
    /// so a wrong offset is a loud failure rather than a stray poke at
    /// whatever is mapped next door.
    #[inline]
    pub fn reg<T: MmioValue>(self, offset: usize) -> MmioReg<T> {
        assert!(
            offset + size_of::<T>() <= self.len,
            "mmio: register past end of device window"
        );
        let addr = self.base.offset(offset);
        assert!(
            addr.is_aligned_to(align_of::<T>()),
            "mmio: misaligned device register"
        );
        MmioReg {
            addr,
            _marker: PhantomData,
        }
    }

    /// Sub-window starting at `offset`, for devices with repeated register
    /// blocks such as a PLIC's per-context enable bitmaps.
    #[inline]
    pub fn subregion(self, offset: usize, len: usize) -> MmioRegion {
        assert!(
            offset + len <= self.len,
            "mmio: subregion past end of device window"
        );
        MmioRegion {
            base: self.base.offset(offset),
            len,
        }
    }
}

/// Scalar widths a device register can have.
///
/// # Safety
/// Implementors must be plain integers with no padding or invalid bit
/// patterns, so that any value the device presents is a valid `Self`.
pub unsafe trait MmioValue: Copy {}

// SAFETY: every bit pattern of these types is a valid value.
unsafe impl MmioValue for u8 {}
// SAFETY: as above.
unsafe impl MmioValue for u16 {}
// SAFETY: as above.
unsafe impl MmioValue for u32 {}
// SAFETY: as above.
unsafe impl MmioValue for u64 {}

/// A single device register.
#[derive(Copy, Clone, Debug)]
pub struct MmioReg<T: MmioValue> {
    addr: Kva,
    _marker: PhantomData<T>,
}

impl<T: MmioValue> MmioReg<T> {
    #[inline]
    pub fn read(self) -> T {
        // SAFETY: `MmioRegion` promised the address is a mapped device
        // register, and `reg` checked that this access is in bounds and
        // aligned. `MmioValue` promises any bit pattern read is a valid `T`.
        unsafe { core::ptr::read_volatile(self.addr.as_ptr::<T>()) }
    }

    #[inline]
    pub fn write(self, value: T) {
        // SAFETY: as `read`; the address is a mapped, in-bounds, aligned
        // device register, so the write goes to the device and nowhere else.
        unsafe { core::ptr::write_volatile(self.addr.as_ptr::<T>(), value) }
    }

    /// Read, transform, write back. Not atomic: callers needing atomicity
    /// against other cores must hold whatever lock guards the device.
    #[inline]
    pub fn modify(self, op: impl FnOnce(T) -> T) {
        self.write(op(self.read()));
    }

    #[inline]
    pub fn addr(self) -> Kva {
        self.addr
    }
}
