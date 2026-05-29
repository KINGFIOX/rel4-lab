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
///
/// Mirrors `write_slot` from `kernel/src/object/cnode.c`: all initial
/// boot-time caps have `revocable = true` and `first_badged = true` so
/// that the CDT walker treats them as legitimate roots (and so derived
/// caps register as proper children via `is_mdb_parent_of`).
pub fn install_initial_cap(cnode: &mut [Cte], i: usize, cap: Cap) {
    assert!(i < cnode.len(), "slot index out of range");
    assert!(cnode[i].cap.is_null(), "slot {} is already populated", i);
    cnode[i].cap = cap;
    let mut mdb = MdbNode::NULL;
    mdb.set_revocable(true);
    mdb.set_first_badged(true);
    cnode[i].mdb = mdb;
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

/// Does `cte` have any CDT children?
///
/// In the seL4 model the MDB linked list is just an ordering over caps
/// — being adjacent in the list does NOT imply parent/child. A node
/// counts as a *child* of `cte` only when:
///   1. The list link goes directly forward from `cte` (next != 0 and
///      next.prev points back at `cte`).
///   2. `isMDBParentOf(cte, next)` holds: `cte` is revocable AND the
///      two caps refer to the same region (e.g. same frame, same EP,
///      or the child's range is contained within `cte`'s untyped).
pub unsafe fn mdb_has_children(cte: *mut Cte) -> bool {
    debug_assert!(!cte.is_null());
    let next_raw = unsafe { (*cte).mdb.next() };
    if next_raw == 0 {
        return false;
    }
    let next = next_raw as *mut Cte;
    if unsafe { (*next).mdb.prev() } != cte as u64 {
        return false;
    }
    unsafe { is_mdb_parent_of(cte, next) }
}

/// Mirrors C kernel `isMDBParentOf(cte_a, cte_b)`: parent must be
/// revocable and the two caps must overlap on the same region. We omit
/// the Endpoint/Notification badge rules for now — neither IPC EP/Ntfn
/// chains nor IRQHandler descendants are exercised yet.
unsafe fn is_mdb_parent_of(a: *mut Cte, b: *mut Cte) -> bool {
    use crate::object::cap::CapTag;
    let a_mdb = unsafe { (*a).mdb };
    if !a_mdb.revocable() {
        return false;
    }
    let cap_a = unsafe { (*a).cap };
    let cap_b = unsafe { (*b).cap };
    let tag_a = cap_a.tag();
    let tag_b = cap_b.tag();
    if tag_a.is_none() || tag_b.is_none() {
        return false;
    }
    let tag_a = tag_a.unwrap();
    let tag_b = tag_b.unwrap();

    match tag_a {
        CapTag::Untyped => {
            // Child must be a physical cap whose backing region lies
            // entirely within `a`'s untyped block.
            let a_base = cap_a.untyped_ptr();
            let a_top = a_base + (1u64 << cap_a.untyped_block_size_bits()) - 1;
            let (b_base, b_size_bits) = match tag_b {
                CapTag::Untyped => (cap_b.untyped_ptr(), cap_b.untyped_block_size_bits()),
                CapTag::CNode => {
                    let bits = cap_b.cnode_radix() + crate::abi::constants::SEL4_SLOT_BITS as u64;
                    (cap_b.cnode_ptr(), bits)
                }
                CapTag::Frame => {
                    // 4 KiB / 2 MiB / 1 GiB frame.
                    let bits = match cap_b.frame_size() {
                        0 => 12,
                        1 => 21,
                        2 => 30,
                        _ => return false,
                    };
                    (cap_b.frame_base_ptr(), bits)
                }
                CapTag::PageTable => (cap_b.page_table_base_ptr(), 12),
                CapTag::Thread => (cap_b.thread_ptr(), 11),
                CapTag::Endpoint => (cap_b.endpoint_ptr(), 4),
                CapTag::Notification => (cap_b.notification_ptr(), 6),
                _ => return false,
            };
            let b_top = b_base + (1u64 << b_size_bits) - 1;
            a_base <= b_base && b_top <= a_top
        }
        CapTag::Endpoint => {
            tag_b == CapTag::Endpoint && cap_a.endpoint_ptr() == cap_b.endpoint_ptr()
        }
        CapTag::Notification => {
            tag_b == CapTag::Notification && cap_a.notification_ptr() == cap_b.notification_ptr()
        }
        CapTag::CNode => {
            tag_b == CapTag::CNode
                && cap_a.cnode_ptr() == cap_b.cnode_ptr()
                && cap_a.cnode_radix() == cap_b.cnode_radix()
        }
        CapTag::Thread => tag_b == CapTag::Thread && cap_a.thread_ptr() == cap_b.thread_ptr(),
        CapTag::Frame => {
            tag_b == CapTag::Frame
                && cap_a.frame_base_ptr() == cap_b.frame_base_ptr()
                && cap_a.frame_size() == cap_b.frame_size()
        }
        CapTag::PageTable => {
            tag_b == CapTag::PageTable && cap_a.page_table_base_ptr() == cap_b.page_table_base_ptr()
        }
        // Domain / IrqControl / AsidControl / AsidPool: there's no real
        // tree below them yet — treat as "no children" for M3.
        _ => false,
    }
}

/// Zero a freshly allocated CNode.
pub unsafe fn zero_cnode(base: *mut u8, radix: usize) {
    unsafe { ptr::write_bytes(base, 0, cnode_bytes(radix)) };
}
