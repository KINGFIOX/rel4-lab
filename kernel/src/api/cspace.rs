//! Capability lookup through a CSpace.
//!
//! A capability pointer (CPtr) is a `seL4_Word` whose bits are partitioned
//! by the chain of CNode caps it must traverse:
//!
//! ```text
//!   MSB                                                            LSB
//!   ┌──────────── 64 bits ─────────────┐
//!   │ guard │ radix │ guard │ radix │…│
//!   └──────────────────────────────────┘
//! ```
//!
//! Each step strips off `guard_size + radix` bits from the *top* of the
//! remaining CPtr, asserts the guard bits match the cap's guard, and
//! uses the `radix` bits as an index into the CNode. If the resolved
//! slot holds another CNode cap, recursion continues; otherwise the
//! lookup terminates with the cap and remaining bits.
//!
//! The implementation walks nested CNodes iteratively under the CSpace lock,
//! matching seL4's depth-limited lookup model without using recursion.

#![allow(dead_code)]

use crate::api::thread::Thread;
use crate::object::cap::{Cap, CapTag};
use crate::object::cnode::{self, Cte};

/// Maximum bits in a CPtr (= `seL4_WordBits`).
pub const WORD_BITS: u32 = 64;

#[derive(Copy, Clone, Debug)]
pub enum LookupError {
    /// Guard bits in the CPtr didn't match the CNode cap's guard.
    GuardMismatch,
    /// CPtr was longer than the cumulative depth of CNode caps walked.
    DepthMismatch,
    /// Resolved slot holds a non-CNode cap mid-walk; this is the standard
    /// terminator for `lookup_slot`, not an error per se.
    Found,
}

#[derive(Copy, Clone, Debug)]
pub struct LookupResult {
    /// Pointer to the `Cte` for the final slot. Always valid when this
    /// struct is returned.
    pub slot: *mut Cte,
    /// Bits of `cptr` left after the walk (0 for a normal lookup; non-zero
    /// when a partial lookup was requested).
    pub bits_remaining: u32,
}

/// Walk the thread's CSpace to resolve `cptr` to a slot pointer.
///
/// `depth_limit` is the number of bits of the CPtr that must be consumed.
/// Pass `WORD_BITS = 64` for normal lookups; smaller values support
/// the partial-CNode-walk operations used by `CNode_Copy` & friends.
pub fn lookup_slot(
    thread: &Thread,
    cptr: u64,
    depth_limit: u32,
) -> Result<LookupResult, LookupError> {
    debug_assert!(!thread.cspace_root.is_null(), "no CSpace installed");
    let root_cap = Cap::new_cnode(
        thread.cspace_root as u64,
        thread.cspace_radix as u64,
        thread.cspace_guard,
        thread.cspace_guard_bits as u64,
    );
    lookup_slot_in(root_cap, cptr, depth_limit)
}

/// Convenience: resolve `cptr` through the thread's CSpace and return the
/// cap stored in the final slot, plus a pointer to the slot itself.
///
/// Mirrors the C kernel's `lookupCap` semantics: a partial walk (where
/// the cptr extends past the deepest CNode in the chain) is a
/// `DepthMismatch` failure, not a successful return of a null cap.
/// Callers that want partial walks (CNode_Copy etc.) should use
/// `lookup_slot_in` directly.
pub fn lookup_cap(thread: &Thread, cptr: u64) -> Result<(Cap, *mut Cte), LookupError> {
    debug_assert!(!thread.cspace_root.is_null(), "no CSpace installed");
    let root_cap = Cap::new_cnode(
        thread.cspace_root as u64,
        thread.cspace_radix as u64,
        thread.cspace_guard,
        thread.cspace_guard_bits as u64,
    );
    lookup_cap_in(root_cap, cptr, WORD_BITS)
}

/// Walk from an explicit CNode cap and snapshot the final cap under the
/// same CSpace lock as the walk.
pub fn lookup_cap_in(
    root_cap: Cap,
    cptr: u64,
    depth_limit: u32,
) -> Result<(Cap, *mut Cte), LookupError> {
    let _cspace_guard = cnode::lock_cspace();
    unsafe {
        let r = lookup_slot_in_locked(root_cap, cptr, depth_limit)?;
        if r.bits_remaining != 0 {
            return Err(LookupError::DepthMismatch);
        }
        let cap = if r.slot.is_null() {
            Cap::null()
        } else {
            (*r.slot).cap
        };
        Ok((cap, r.slot))
    }
}

/// Walk a CSpace whose root is the given CNode `cap`, resolving `cptr`
/// using `depth_limit` bits. Supports nested CNode walks (a slot
/// containing another CNode cap consumes more bits) but does not
/// recurse into non-CNode caps. The whole walk runs under the CSpace
/// lock so nested CNode descent sees a consistent CTE chain.
pub fn lookup_slot_in(cap: Cap, cptr: u64, depth_limit: u32) -> Result<LookupResult, LookupError> {
    let _cspace_guard = cnode::lock_cspace();
    unsafe { lookup_slot_in_locked(cap, cptr, depth_limit) }
}

unsafe fn lookup_slot_in_locked(
    mut cap: Cap,
    mut cptr: u64,
    mut depth_limit: u32,
) -> Result<LookupResult, LookupError> {
    loop {
        if cap.tag() != Some(CapTag::CNode) {
            return Err(LookupError::DepthMismatch);
        }
        let radix = cap.cnode_radix() as u32;
        let guard_bits = cap.cnode_guard_size() as u32;
        let guard = cap.cnode_guard();
        let total = radix + guard_bits;
        if depth_limit < total {
            return Err(LookupError::DepthMismatch);
        }

        // Top of the CPtr after the depth window: peel `total` bits.
        let cptr_top = cptr >> (depth_limit - total);
        let guard_mask = if guard_bits == 0 {
            0
        } else {
            (1u64 << guard_bits) - 1
        };
        if ((cptr_top >> radix) & guard_mask) != (guard & guard_mask) {
            return Err(LookupError::GuardMismatch);
        }
        let radix_mask = (1u64 << radix) - 1;
        let idx = cptr_top & radix_mask;
        let cnode_base = cap.cnode_ptr() as *mut Cte;
        let slot = unsafe { cnode_base.add(idx as usize) };

        let remaining = depth_limit - total;
        if remaining == 0 {
            return Ok(LookupResult {
                slot,
                bits_remaining: 0,
            });
        }

        // More bits to resolve — descend through the slot's cap if it's
        // another CNode, otherwise stop here.
        let next_cap = unsafe { (*slot).cap };
        if next_cap.tag() != Some(CapTag::CNode) {
            return Ok(LookupResult {
                slot,
                bits_remaining: remaining,
            });
        }
        // Strip the bits we just consumed and recurse.
        cptr &= (1u64 << (depth_limit - total)) - 1;
        depth_limit -= total;
        cap = next_cap;
    }
}
