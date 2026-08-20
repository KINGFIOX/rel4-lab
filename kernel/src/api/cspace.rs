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
//! The implementation walks nested CNodes iteratively, matching seL4's
//! depth-limited lookup model without using recursion. A thread's CSpace root
//! is simply the CNode cap in its CTable slot, so lookups take the thread
//! whose CSpace to walk rather than a cached copy of its root.

#![allow(dead_code)]
// This module is written entirely in terms of the safe abstractions in
// `ktypes`; keep it that way.
#![deny(unsafe_code)]

use crate::object::cap::{Cap, CapTag};
use crate::object::cnode::CteRef;
use crate::object::tcb::{self, TcbRef};

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
    /// The `Cte` for the final slot.
    pub slot: CteRef,
    /// Bits of `cptr` left after the walk (0 for a normal lookup; non-zero
    /// when a partial lookup was requested).
    pub bits_remaining: u32,
}

/// The CSpace root cap of `tcb`.
#[inline]
pub fn root_cap_of(tcb: TcbRef) -> Cap {
    tcb.cspace_cap()
}

/// Walk `tcb`'s CSpace to resolve `cptr` to a slot.
///
/// `depth_limit` is the number of bits of the CPtr that must be consumed.
/// Pass `WORD_BITS = 64` for normal lookups; smaller values support the
/// partial-CNode-walk operations used by `CNode_Copy` and friends.
pub fn lookup_slot(tcb: TcbRef, cptr: u64, depth_limit: u32) -> Result<LookupResult, LookupError> {
    lookup_slot_in(root_cap_of(tcb), cptr, depth_limit)
}

/// Resolve `cptr` through `tcb`'s CSpace and return the cap stored in the
/// final slot, plus the slot itself.
///
/// Mirrors the C kernel's `lookupCap` semantics: a partial walk (where the
/// cptr extends past the deepest CNode in the chain) is a `DepthMismatch`
/// failure, not a successful return of a null cap. Callers that want partial
/// walks (CNode_Copy etc.) should use `lookup_slot_in` directly.
pub fn lookup_cap(tcb: TcbRef, cptr: u64) -> Result<(Cap, CteRef), LookupError> {
    lookup_cap_in(root_cap_of(tcb), cptr, WORD_BITS)
}

/// As `lookup_cap`, for the thread the local core is running.
pub fn lookup_cap_current(cptr: u64) -> Result<(Cap, CteRef), LookupError> {
    match tcb::current() {
        Some(tcb) => lookup_cap(tcb, cptr),
        None => Err(LookupError::DepthMismatch),
    }
}

/// Walk from an explicit CNode cap and read the final cap.
pub fn lookup_cap_in(
    root_cap: Cap,
    cptr: u64,
    depth_limit: u32,
) -> Result<(Cap, CteRef), LookupError> {
    let result = lookup_slot_in(root_cap, cptr, depth_limit)?;
    if result.bits_remaining != 0 {
        return Err(LookupError::DepthMismatch);
    }
    Ok((result.slot.cap(), result.slot))
}

/// Walk a CSpace whose root is the given CNode `cap`, resolving `cptr`
/// using `depth_limit` bits. Supports nested CNode walks (a slot
/// containing another CNode cap consumes more bits) but does not
/// recurse into non-CNode caps.
pub fn lookup_slot_in(
    mut cap: Cap,
    mut cptr: u64,
    mut depth_limit: u32,
) -> Result<LookupResult, LookupError> {
    loop {
        let Some(cnode) = cap.as_cnode() else {
            return Err(LookupError::DepthMismatch);
        };
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
        let index = (cptr_top & radix_mask) as usize;
        let slot = cnode
            .get(index)
            .expect("cptr index is masked to the CNode's radix");

        let remaining = depth_limit - total;
        if remaining == 0 {
            return Ok(LookupResult {
                slot,
                bits_remaining: 0,
            });
        }

        // More bits to resolve — descend through the slot's cap if it's
        // another CNode, otherwise stop here.
        let next_cap = slot.cap();
        if next_cap.tag() != Some(CapTag::CNode) {
            return Ok(LookupResult {
                slot,
                bits_remaining: remaining,
            });
        }
        // Strip the bits we just consumed and continue.
        cptr &= (1u64 << (depth_limit - total)) - 1;
        depth_limit -= total;
        cap = next_cap;
    }
}
