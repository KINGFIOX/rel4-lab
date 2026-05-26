//! CNode (capability node) — a flat array of `cte_t` slots.
//!
//! A CNode of radix `r` has `2^r` slots, each `sizeof(cte_t) = 32` bytes.
//! Slot 0 is the null cap by convention. The root CNode for our build is
//! `radix = CONFIG_ROOT_CNODE_SIZE_BITS = 13`, i.e. 8192 slots = 256 KiB.

#![allow(dead_code)]

use core::ptr;

use crate::abi::constants::SEL4_SLOT_BITS;
use crate::object::cap::Cap;
use crate::object::mdb::MdbNode;

/// Capability table entry — one slot of a CNode.
///
/// Adjacent in memory: the cap itself, then its MDB linkage to other
/// derived/copied caps. Total size must be `1 << seL4_SlotBits = 32`.
#[repr(C)]
#[derive(Copy, Clone, Debug, Default)]
pub struct Cte {
    pub cap: Cap,
    pub mdb: MdbNode,
}

const _: () = {
    assert!(core::mem::size_of::<Cte>() == 1 << SEL4_SLOT_BITS);
    assert!(core::mem::align_of::<Cte>() >= 8);
};

/// Bytes occupied by a CNode of `radix` bits.
#[inline]
pub const fn cnode_bytes(radix: usize) -> usize {
    (1usize << radix) * core::mem::size_of::<Cte>()
}

/// View a contiguous kernel-allocated memory region as a CNode of `radix`
/// bits. The caller must guarantee that `base` is suitably aligned and
/// at least `cnode_bytes(radix)` long.
pub unsafe fn cnode_at(base: *mut u8, radix: usize) -> &'static mut [Cte] {
    debug_assert!((base as usize) & 0xF == 0, "CNode must be 16-byte aligned");
    let len = 1usize << radix;
    unsafe { core::slice::from_raw_parts_mut(base as *mut Cte, len) }
}

/// Install `cap` (with empty MDB linkage) at slot `i` of `cnode`. Panics
/// if the slot is non-empty.
pub fn install_initial_cap(cnode: &mut [Cte], i: usize, cap: Cap) {
    assert!(i < cnode.len(), "slot index out of range");
    assert!(cnode[i].cap.is_null(), "slot {} is already populated", i);
    cnode[i].cap = cap;
    cnode[i].mdb = MdbNode::NULL;
}

/// Zero a freshly allocated CNode.
pub unsafe fn zero_cnode(base: *mut u8, radix: usize) {
    unsafe { ptr::write_bytes(base, 0, cnode_bytes(radix)) };
}
