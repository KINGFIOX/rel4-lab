//! Handles for kernel objects that live in user-retyped memory.
//!
//! seL4 objects are not owned by the kernel in the Rust sense: user code
//! retypes an Untyped, the kernel stamps an object into the resulting slab,
//! and a capability keeps the object alive until the Untyped is revoked. There
//! is no single owner a Rust reference could borrow from, and objects point at
//! each other (a TCB names the endpoint it blocks on, an endpoint names the
//! TCBs queued on it), so the object graph cannot be expressed as a tree of
//! `&mut`.
//!
//! [`ObjRef`] is the honest representation of that: a non-null, correctly
//! aligned address of a live object, carrying the object's type. Two things
//! make it better than the raw pointer it replaces:
//!
//! * it cannot be null, so the "is this pointer real?" checks that used to be
//!   duplicated at every call site collapse into `Option<ObjRef<T>>`, and
//!   `Option<ObjRef<T>>` still occupies exactly one word, so object layouts
//!   that store these as KVAs keep their size and field offsets;
//! * dereferencing only happens inside [`ObjRef::with`] and
//!   [`ObjRef::with_mut`], which assert the big kernel lock is held. Callers
//!   get an ordinary `&T`/`&mut T` for the duration of a closure and need no
//!   `unsafe` of their own.

use core::cell::UnsafeCell;
use core::marker::PhantomData;
use core::ptr::NonNull;

use crate::kernel::smp::debug_assert_kernel_lock_held;

/// A live kernel object of type `T`, addressed through the kernel window.
///
/// `ObjRef` is `Copy` and carries no lifetime: object lifetime is governed by
/// capabilities and the capability derivation tree, which the borrow checker
/// cannot model. Holding one asserts the object was live when the handle was
/// created; code that can destroy objects (`finalize`, revoke) is responsible
/// for not handing stale handles onwards.
#[repr(transparent)]
pub struct ObjRef<T> {
    ptr: NonNull<T>,
    _marker: PhantomData<*mut T>,
}

impl<T> Clone for ObjRef<T> {
    fn clone(&self) -> Self {
        *self
    }
}

impl<T> Copy for ObjRef<T> {}

// SAFETY: a handle is an address in the kernel window, which every core
// interprets the same way, and every access through it takes the big kernel
// lock. Sending or sharing the handle itself therefore adds no races beyond
// the ones that lock already governs.
unsafe impl<T> Send for ObjRef<T> {}
// SAFETY: as above.
unsafe impl<T> Sync for ObjRef<T> {}

impl<T> PartialEq for ObjRef<T> {
    fn eq(&self, other: &Self) -> bool {
        self.ptr == other.ptr
    }
}

impl<T> Eq for ObjRef<T> {}

impl<T> core::fmt::Debug for ObjRef<T> {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        write!(f, "ObjRef({:#x})", self.kva())
    }
}

impl<T> ObjRef<T> {
    /// Build a handle from a kernel-window address.
    ///
    /// # Safety
    /// `kva` must be the base address of a live `T` reachable through the
    /// kernel window, aligned for `T`, and it must stay live for as long as
    /// the handle is used. In practice this holds for addresses the kernel
    /// itself wrote: object bases produced by `Untyped_Retype`, addresses read
    /// back out of a live capability, and the kernel's own static objects.
    #[inline]
    pub const unsafe fn from_kva_unchecked(kva: u64) -> Self {
        // SAFETY: caller guarantees `kva` is the non-null base of a live `T`.
        let ptr = unsafe { NonNull::new_unchecked(kva as *mut T) };
        Self {
            ptr,
            _marker: PhantomData,
        }
    }

    /// Build a handle from a possibly-zero kernel-window address.
    ///
    /// Object fields store "no object" as address zero, so this is the usual
    /// way to decode a stored link or a capability's pointer field.
    ///
    /// # Safety
    /// If `kva` is non-zero it must satisfy the obligations of
    /// [`ObjRef::from_kva_unchecked`].
    #[inline]
    pub const unsafe fn from_kva(kva: u64) -> Option<Self> {
        if kva == 0 {
            None
        } else {
            // SAFETY: non-zero, and the caller vouches for the rest.
            Some(unsafe { Self::from_kva_unchecked(kva) })
        }
    }

    /// Build a handle for one of the kernel's own statically allocated
    /// objects, borrowing its address from an existing reference.
    ///
    /// # Safety
    /// The pointee must live for the whole time the handle is used, and all
    /// accesses to it must go through handles from then on, so that the
    /// aliasing rules of [`ObjRef::with_mut`] are not broken by an unrelated
    /// long-lived reference.
    #[inline]
    pub const unsafe fn from_ptr(ptr: *mut T) -> Option<Self> {
        match NonNull::new(ptr) {
            None => None,
            Some(ptr) => Some(Self {
                ptr,
                _marker: PhantomData,
            }),
        }
    }

    /// Kernel-window address of the object, as stored in caps and in object
    /// link fields.
    #[inline]
    pub fn kva(self) -> u64 {
        self.ptr.as_ptr() as u64
    }

    /// Raw pointer to the object, for the few places that must hand an address
    /// to assembly or to a hardware register.
    #[inline]
    pub fn as_ptr(self) -> *mut T {
        self.ptr.as_ptr()
    }

    /// Reinterpret the handle as addressing a different object type.
    ///
    /// # Safety
    /// The address must be the base of a live `U`.
    #[inline]
    pub unsafe fn cast<U>(self) -> ObjRef<U> {
        // SAFETY: caller guarantees the address is a live `U`; non-nullness is
        // inherited from `self`.
        unsafe { ObjRef::from_kva_unchecked(self.kva()) }
    }

    /// Borrow the object for the duration of `op`.
    ///
    /// Requires the big kernel lock, which is what serialises kernel object
    /// access against the other cores.
    #[inline]
    pub fn with<R>(self, op: impl FnOnce(&T) -> R) -> R {
        debug_assert_kernel_lock_held();
        // SAFETY: the handle addresses a live, aligned `T` (invariant of
        // `ObjRef`), and the big kernel lock keeps other cores out for the
        // duration of the borrow.
        op(unsafe { self.ptr.as_ref() })
    }

    /// Borrow the object mutably for the duration of `op`.
    ///
    /// Requires the big kernel lock. Do not nest `with_mut` on the same
    /// object; use [`ObjRef::with_pair_mut`] when two objects of the same type
    /// have to be updated together, since it rejects the aliasing case.
    #[inline]
    pub fn with_mut<R>(mut self, op: impl FnOnce(&mut T) -> R) -> R {
        debug_assert_kernel_lock_held();
        // SAFETY: the handle addresses a live, aligned `T` (invariant of
        // `ObjRef`), and the big kernel lock keeps other cores out for the
        // duration of the borrow. The borrow ends with `op`, so it cannot
        // overlap a later one on this core.
        op(unsafe { self.ptr.as_mut() })
    }

    /// Borrow two distinct objects mutably at once, for operations such as an
    /// IPC transfer that must touch both endpoints of the rendezvous.
    ///
    /// Returns `None` when both handles name the same object, since that would
    /// alias.
    #[inline]
    pub fn with_pair_mut<R>(
        mut first: Self,
        mut second: Self,
        op: impl FnOnce(&mut T, &mut T) -> R,
    ) -> Option<R> {
        debug_assert_kernel_lock_held();
        if first == second {
            return None;
        }
        // SAFETY: both handles address live, aligned `T`s, the equality check
        // above proves the two borrows do not overlap, and the big kernel lock
        // keeps other cores out for the duration.
        Some(op(unsafe { first.ptr.as_mut() }, unsafe {
            second.ptr.as_mut()
        }))
    }
}

/// Optional-handle helpers, so callers can keep working in terms of the stored
/// address without reaching for a raw pointer.
pub trait OptObjRefExt {
    /// Stored representation: the object's KVA, or zero for "no object".
    fn kva_or_zero(self) -> u64;
}

impl<T> OptObjRefExt for Option<ObjRef<T>> {
    #[inline]
    fn kva_or_zero(self) -> u64 {
        match self {
            Some(r) => r.kva(),
            None => 0,
        }
    }
}

/// A contiguous run of `len` objects starting at a base handle, used for
/// capability tables and for the CTEs embedded in a TCB.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub struct ObjArray<T> {
    base: ObjRef<T>,
    len: usize,
}

impl<T> ObjArray<T> {
    /// Describe `len` consecutive `T`s starting at `base`.
    ///
    /// # Safety
    /// All `len` objects must be live and contiguous, and the whole run must
    /// stay within one allocation so that pointer arithmetic over it is
    /// in-bounds.
    #[inline]
    pub const unsafe fn new(base: ObjRef<T>, len: usize) -> Self {
        Self { base, len }
    }

    #[inline]
    pub fn len(self) -> usize {
        self.len
    }

    #[inline]
    pub fn is_empty(self) -> bool {
        self.len == 0
    }

    #[inline]
    pub fn base(self) -> ObjRef<T> {
        self.base
    }

    /// Handle for element `index`, or `None` when out of range.
    #[inline]
    pub fn get(self, index: usize) -> Option<ObjRef<T>> {
        if index >= self.len {
            return None;
        }
        // SAFETY: `index` is in range, so the offset stays inside the run the
        // constructor promised is one contiguous allocation of live `T`s.
        let ptr = unsafe { self.base.as_ptr().add(index) };
        // SAFETY: offsetting a non-null in-bounds pointer keeps it non-null.
        unsafe { ObjRef::from_ptr(ptr) }
    }

    /// Borrow the whole run as a slice for the duration of `op`.
    #[inline]
    pub fn with_slice<R>(self, op: impl FnOnce(&[T]) -> R) -> R {
        debug_assert_kernel_lock_held();
        // SAFETY: constructor guarantees `len` contiguous live `T`s, and the
        // big kernel lock serialises access for the duration of the borrow.
        op(unsafe { core::slice::from_raw_parts(self.base.as_ptr(), self.len) })
    }

    /// Borrow the whole run mutably as a slice for the duration of `op`.
    #[inline]
    pub fn with_slice_mut<R>(self, op: impl FnOnce(&mut [T]) -> R) -> R {
        debug_assert_kernel_lock_held();
        // SAFETY: as `with_slice`, and the borrow ends with `op` so it cannot
        // overlap another borrow on this core.
        op(unsafe { core::slice::from_raw_parts_mut(self.base.as_ptr(), self.len) })
    }
}

/// A kernel object that lives in a `static` rather than in user-retyped
/// memory: the per-core idle threads and the rootserver TCB.
///
/// Such an object is reached the same way as any other, through an
/// [`ObjRef`], so the rest of the kernel does not care where it came from.
pub struct ObjCell<T> {
    value: UnsafeCell<T>,
}

// SAFETY: the object is only reachable through the `ObjRef` handed out by
// `get`, and those accesses require the big kernel lock.
unsafe impl<T: Send> Sync for ObjCell<T> {}

impl<T> ObjCell<T> {
    pub const fn new(value: T) -> Self {
        Self {
            value: UnsafeCell::new(value),
        }
    }

    /// Handle for the contained object.
    #[inline]
    pub fn get(&'static self) -> ObjRef<T> {
        // SAFETY: the cell is `'static`, so the object outlives every handle,
        // its address is non-null and correctly aligned, and the cell exposes
        // no other way to reach the value, so all accesses go through handles.
        unsafe { ObjRef::from_kva_unchecked(self.value.get() as u64) }
    }
}

const _: () = {
    // Object layouts store links as one word; the handle must not be fatter.
    assert!(size_of::<Option<ObjRef<u8>>>() == size_of::<u64>());
    assert!(align_of::<Option<ObjRef<u8>>>() == align_of::<u64>());
};
