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

/// Insert `new_cte` (already populated with cap + initial MDB) right
/// after `parent` in the CDT doubly-linked list. Both pointers must be
/// non-null and not equal.
pub unsafe fn mdb_insert_after(parent: *mut Cte, new_cte: *mut Cte) {
    debug_assert!(!parent.is_null() && !new_cte.is_null() && parent != new_cte);
    unsafe {
        let parent_next = (*parent).mdb.next();
        (*new_cte).mdb.set_prev(parent as u64);
        (*new_cte).mdb.set_next(parent_next);
        (*parent).mdb.set_next(new_cte as u64);
        if parent_next != 0 {
            let next = parent_next as *mut Cte;
            (*next).mdb.set_prev(new_cte as u64);
        }
    }
}

/// Unlink `cte` from its CDT siblings. Leaves the slot otherwise intact.
pub unsafe fn mdb_unlink(cte: *mut Cte) {
    debug_assert!(!cte.is_null());
    unsafe {
        let prev = (*cte).mdb.prev();
        let next = (*cte).mdb.next();
        if prev != 0 {
            let p = prev as *mut Cte;
            (*p).mdb.set_next(next);
        }
        if next != 0 {
            let n = next as *mut Cte;
            (*n).mdb.set_prev(prev);
        }
        (*cte).mdb = MdbNode::NULL;
    }
}

/// Does `cte` have any CDT children, i.e. anything whose prev points to it?
pub unsafe fn mdb_has_children(cte: *mut Cte) -> bool {
    debug_assert!(!cte.is_null());
    let next = unsafe { (*cte).mdb.next() };
    if next == 0 {
        return false;
    }
    let n = next as *mut Cte;
    unsafe { (*n).mdb.prev() == cte as u64 }
}

/// Zero a freshly allocated CNode.
pub unsafe fn zero_cnode(base: *mut u8, radix: usize) {
    unsafe { ptr::write_bytes(base, 0, cnode_bytes(radix)) };
}
