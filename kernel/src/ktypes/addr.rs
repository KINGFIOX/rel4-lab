//! Address newtypes.
//!
//! The kernel juggles three address spaces: physical addresses that go into
//! page-table entries and hardware registers, kernel-window addresses that the
//! kernel dereferences directly, and user virtual addresses that only mean
//! something relative to some VSpace. All three used to be `usize`, which made
//! a swapped argument a silent bug. Wrapping them keeps the arithmetic while
//! making the conversions explicit and checkable.
//!
//! The conversions between them are platform layout decisions, so they live in
//! `arch::current::object::vspace` rather than here.

// This module is written entirely in terms of the safe abstractions in
// `ktypes`; keep it that way.
#![deny(unsafe_code)]

/// A physical address.
#[repr(transparent)]
#[derive(Copy, Clone, PartialEq, Eq, PartialOrd, Ord, Default)]
pub struct Paddr(usize);

/// An address in the kernel's identity window (seL4's PSpace / `pptr`), i.e.
/// something the kernel may dereference once paging is up.
#[repr(transparent)]
#[derive(Copy, Clone, PartialEq, Eq, PartialOrd, Ord, Default)]
pub struct Kva(usize);

/// A virtual address in some user VSpace. Only meaningful together with the
/// VSpace root it belongs to; never dereferenced by the kernel.
#[repr(transparent)]
#[derive(Copy, Clone, PartialEq, Eq, PartialOrd, Ord, Default)]
pub struct UserVa(usize);

macro_rules! impl_addr {
    ($name:ident, $tag:literal) => {
        impl $name {
            pub const ZERO: Self = Self(0);

            #[inline]
            pub const fn new(raw: usize) -> Self {
                Self(raw)
            }

            #[inline]
            pub const fn from_u64(raw: u64) -> Self {
                Self(raw as usize)
            }

            #[inline]
            pub const fn raw(self) -> usize {
                self.0
            }

            #[inline]
            pub const fn as_u64(self) -> u64 {
                self.0 as u64
            }

            #[inline]
            pub const fn is_zero(self) -> bool {
                self.0 == 0
            }

            #[inline]
            pub const fn offset(self, bytes: usize) -> Self {
                Self(self.0 + bytes)
            }

            /// Distance from `base` to `self`, saturating at zero.
            #[inline]
            pub const fn bytes_from(self, base: Self) -> usize {
                self.0.saturating_sub(base.0)
            }

            #[inline]
            pub const fn is_aligned_to(self, align: usize) -> bool {
                self.0 & (align - 1) == 0
            }

            #[inline]
            pub const fn align_down(self, align: usize) -> Self {
                Self(self.0 & !(align - 1))
            }

            #[inline]
            pub const fn align_up(self, align: usize) -> Self {
                Self((self.0 + align - 1) & !(align - 1))
            }
        }

        impl core::fmt::Debug for $name {
            fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
                write!(f, concat!($tag, "({:#x})"), self.0)
            }
        }

        impl core::fmt::LowerHex for $name {
            fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
                core::fmt::LowerHex::fmt(&self.0, f)
            }
        }
    };
}

impl_addr!(Paddr, "Paddr");
impl_addr!(Kva, "Kva");
impl_addr!(UserVa, "UserVa");

impl Kva {
    /// View the kernel-window address as a pointer.
    ///
    /// Producing the pointer is safe; dereferencing it is the caller's
    /// problem, which is why the kernel object layer goes through
    /// [`crate::ktypes::objref::ObjRef`] instead.
    #[inline]
    pub const fn as_ptr<T>(self) -> *mut T {
        self.0 as *mut T
    }
}
