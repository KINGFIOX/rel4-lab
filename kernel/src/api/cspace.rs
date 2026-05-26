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
//! For M3.1 the rootserver only has a single root CNode and uses CPtrs
//! that fit within its radix, so we implement the single-level case
//! tail-call style with a manual loop (no recursion).

#![allow(dead_code)]

use crate::api::thread::Thread;
use crate::object::cap::{Cap, CapTag};
use crate::object::cnode::Cte;

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

/// Walk the thread's CSpace to resolve `cptr` to a slot pointer. Uses the
/// standard seL4 algorithm but assumes a single root CNode for now.
///
/// `depth_limit` is the number of bits of the CPtr that must be consumed.
/// Pass `seL4_WordBits = 64` for normal lookups; smaller values support
/// the partial-CNode-walk operations used by `CNode_Copy` & friends.
pub fn lookup_slot(
    thread: &Thread,
    cptr: u64,
    depth_limit: u32,
) -> Result<LookupResult, LookupError> {
    debug_assert!(!thread.cspace_root.is_null(), "no CSpace installed");

    // Single-CNode walk: peel off the top `guard_bits` of the CPtr (must
    // equal the cap's guard) and then `radix` bits as the slot index.
    let total = thread.cspace_guard_bits + thread.cspace_radix;
    if depth_limit < total {
        return Err(LookupError::DepthMismatch);
    }

    let cptr_top = cptr >> (depth_limit - total);
    let guard_mask = if thread.cspace_guard_bits == 0 {
        0
    } else {
        (1u64 << thread.cspace_guard_bits) - 1
    };
    let guard_value = (cptr_top >> thread.cspace_radix) & guard_mask;
    if guard_value != (thread.cspace_guard & guard_mask) {
        return Err(LookupError::GuardMismatch);
    }
    let radix_mask = (1u64 << thread.cspace_radix) - 1;
    let idx = cptr_top & radix_mask;
    let slot = unsafe { thread.cspace_root.add(idx as usize) };
    Ok(LookupResult {
        slot,
        bits_remaining: depth_limit - total,
    })
}

/// Convenience: resolve `cptr` and return the cap stored in the slot.
pub fn lookup_cap(
    thread: &Thread,
    cptr: u64,
) -> Result<(Cap, *mut Cte), LookupError> {
    let r = lookup_slot(thread, cptr, 64)?;
    let cap = unsafe { (*r.slot).cap };
    Ok((cap, r.slot))
}

/// Variant used by invocations that need a *specific* cap type. Returns
/// the slot pointer alongside the cap so the caller can mutate the cte.
pub fn lookup_cap_of(thread: &Thread, cptr: u64, want: CapTag) -> Option<(Cap, *mut Cte)> {
    let (cap, slot) = lookup_cap(thread, cptr).ok()?;
    if cap.tag() == Some(want) {
        Some((cap, slot))
    } else {
        None
    }
}
