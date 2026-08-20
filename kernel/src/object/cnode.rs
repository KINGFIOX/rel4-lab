//! CNode (capability node) — a flat array of `cte_t` slots.
//!
//! A CNode of radix `r` has `2^r` slots, each `sizeof(cte_t) = 32` bytes.
//! Slot 0 is the null cap by convention. The root CNode radix is configured
//! by `ROOT_CNODE_SIZE_BITS`.

#![allow(dead_code)]

use crate::abi::constants::{
    SEL4_ASID_POOL_BITS, SEL4_ENDPOINT_BITS, SEL4_MIN_UNTYPED_BITS, SEL4_NOTIFICATION_BITS,
    SEL4_PAGE_TABLE_BITS, SEL4_SLOT_BITS, SEL4_TCB_BITS,
};
use crate::ktypes::objref::{ObjArray, ObjRef};
use crate::object::cap::{Cap, CapTag};
use crate::object::mdb::MdbNode;

/// Handle for one capability slot.
pub type CteRef = ObjRef<Cte>;

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

impl Cte {
    pub const fn null() -> Self {
        Self {
            cap: Cap::null(),
            mdb: MdbNode::NULL,
        }
    }

    #[inline]
    fn is_empty(&self) -> bool {
        self.cap.is_null() && self.mdb.prev() == 0 && self.mdb.next() == 0
    }
}

impl CteRef {
    /// The capability stored in this slot.
    #[inline]
    pub fn cap(self) -> Cap {
        self.with(|cte| cte.cap)
    }

    #[inline]
    pub fn set_cap(self, cap: Cap) {
        self.with_mut(|cte| cte.cap = cap);
    }

    /// Modify the capability in place, for the bookkeeping fields the kernel
    /// updates without replacing the whole cap (an untyped's free index, a
    /// frame's mapping).
    #[inline]
    pub fn update_cap(self, op: impl FnOnce(&mut Cap)) {
        self.with_mut(|cte| op(&mut cte.cap));
    }

    #[inline]
    pub fn mdb(self) -> MdbNode {
        self.with(|cte| cte.mdb)
    }

    #[inline]
    pub fn set_mdb(self, mdb: MdbNode) {
        self.with_mut(|cte| cte.mdb = mdb);
    }

    /// The slot this one derives from, if any.
    #[inline]
    pub fn mdb_prev(self) -> Option<CteRef> {
        // SAFETY: MDB links only ever hold addresses of live CTEs that this
        // module stored.
        unsafe { ObjRef::from_kva(self.mdb().prev()) }
    }

    /// The next slot in capability derivation order, if any.
    #[inline]
    pub fn mdb_next(self) -> Option<CteRef> {
        // SAFETY: as `mdb_prev`.
        unsafe { ObjRef::from_kva(self.mdb().next()) }
    }

    #[inline]
    pub fn set_mdb_prev(self, prev: Option<CteRef>) {
        let kva = prev.map_or(0, ObjRef::kva);
        self.with_mut(|cte| cte.mdb.set_prev(kva));
    }

    #[inline]
    pub fn set_mdb_next(self, next: Option<CteRef>) {
        let kva = next.map_or(0, ObjRef::kva);
        self.with_mut(|cte| cte.mdb.set_next(kva));
    }

    #[inline]
    pub fn is_empty(self) -> bool {
        self.with(Cte::is_empty)
    }

    /// Insert `new_cte` (already populated with cap and initial MDB linkage)
    /// right after this slot in the capability derivation list.
    pub fn mdb_insert_after(self, new_cte: CteRef) {
        assert!(
            self != new_cte,
            "mdbInsertAfter source and destination must differ"
        );
        let next = self.mdb_next();
        new_cte.with_mut(|cte| {
            cte.mdb.set_prev(self.kva());
            cte.mdb.set_next(next.map_or(0, ObjRef::kva));
        });
        self.set_mdb_next(Some(new_cte));
        if let Some(next) = next {
            next.set_mdb_prev(Some(new_cte));
        }
    }

    /// Mirror seL4 `insertNewCap`: publish a freshly-created object cap as a
    /// revocable child immediately after this slot.
    pub fn insert_new_cap(self, slot: CteRef, cap: Cap) {
        assert!(
            self != slot,
            "insertNewCap parent and destination must differ"
        );
        assert!(slot.is_empty(), "insertNewCap to non-empty destination");

        let next = self.mdb_next();
        slot.with_mut(|cte| {
            cte.cap = cap;
            cte.mdb = MdbNode::new(self.kva(), next.map_or(0, ObjRef::kva), true, true);
        });
        if let Some(next) = next {
            next.set_mdb_prev(Some(slot));
        }
        self.set_mdb_next(Some(slot));
    }

    /// Mirror seL4 `cteInsert`: install `new_cap` into the empty destination
    /// slot and link it immediately after this one.
    ///
    /// The caller derives/masks `new_cap` and validates source and destination
    /// lookup rights before insertion.
    pub fn cte_insert(self, new_cap: Cap, dest_slot: CteRef) {
        assert!(
            self != dest_slot,
            "cteInsert source and destination must differ"
        );
        assert!(dest_slot.is_empty(), "cteInsert to non-empty destination");

        let (src_cap, src_mdb) = self.with(|cte| (cte.cap, cte.mdb));
        let new_revocable = is_cap_revocable(new_cap, src_cap);
        let new_mdb = MdbNode::new(self.kva(), src_mdb.next(), new_revocable, new_revocable);

        self.set_untyped_cap_as_full(src_cap, new_cap);
        dest_slot.with_mut(|cte| {
            cte.cap = new_cap;
            cte.mdb = new_mdb;
        });
        self.with_mut(|cte| cte.mdb.set_next(dest_slot.kva()));
        // SAFETY: an MDB link holds the address of a live CTE.
        if let Some(next) = unsafe { ObjRef::<Cte>::from_kva(src_mdb.next()) } {
            next.set_mdb_prev(Some(dest_slot));
        }
    }

    /// When a cap is copied out of an untyped without consuming any of it,
    /// seL4 marks the source untyped as fully allocated.
    fn set_untyped_cap_as_full(self, src_cap: Cap, new_cap: Cap) {
        if src_cap.tag() != Some(CapTag::Untyped) || new_cap.tag() != Some(CapTag::Untyped) {
            return;
        }
        if src_cap.untyped_ptr() != new_cap.untyped_ptr()
            || src_cap.untyped_block_size_bits() != new_cap.untyped_block_size_bits()
        {
            return;
        }
        let size_bits = src_cap.untyped_block_size_bits();
        if size_bits < SEL4_MIN_UNTYPED_BITS as u64 {
            return;
        }
        let free_index = 1u64 << (size_bits - SEL4_MIN_UNTYPED_BITS as u64);
        self.with_mut(|cte| cte.cap.set_untyped_free_index(free_index));
    }

    /// Unlink this slot from its derivation siblings, leaving the slot
    /// otherwise intact.
    pub fn mdb_unlink(self) {
        let prev = self.mdb_prev();
        let next = self.mdb_next();
        if let Some(prev) = prev {
            prev.set_mdb_next(next);
        }
        if let Some(next) = next {
            next.set_mdb_prev(prev);
        }
        self.set_mdb(MdbNode::NULL);
    }

    /// Does this slot have any derivation children?
    ///
    /// In the seL4 model the MDB linked list is just an ordering over caps —
    /// being adjacent in the list does NOT imply parent/child. A node counts
    /// as a *child* only when the direct successor satisfies seL4
    /// `isMDBParentOf`: this slot is revocable AND the two caps refer to the
    /// same region (e.g. same frame, same endpoint, or the child's range is
    /// contained within this untyped).
    pub fn mdb_has_children(self) -> bool {
        match self.mdb_next() {
            Some(next) => self.is_mdb_parent_of(next),
            None => false,
        }
    }

    /// Mirrors C kernel `isMDBParentOf(cte_a, cte_b)`: parent must be
    /// revocable, the two caps must overlap on the same region, and badged
    /// Endpoint/Notification caps only parent same-badge descendants that are
    /// not the first badged cap in a branch.
    fn is_mdb_parent_of(self, other: CteRef) -> bool {
        let (cap_a, a_mdb) = self.with(|cte| (cte.cap, cte.mdb));
        if !a_mdb.revocable() {
            return false;
        }
        let (cap_b, b_first_badged) = other.with(|cte| (cte.cap, cte.mdb.first_badged()));
        let (Some(tag_a), Some(tag_b)) = (cap_a.tag(), cap_b.tag()) else {
            return false;
        };

        match tag_a {
            CapTag::Untyped => {
                // Child must be a physical cap whose backing region lies
                // entirely within `a`'s untyped block.
                let a_base = cap_a.untyped_ptr();
                let a_top = region_top(a_base, cap_a.untyped_block_size_bits());
                let Some((b_base, b_size_bits)) = physical_cap_region(cap_b) else {
                    return false;
                };
                let b_top = region_top(b_base, b_size_bits);
                a_base <= b_base && b_top <= a_top && b_base <= b_top
            }
            CapTag::Endpoint => {
                if tag_b != CapTag::Endpoint || cap_a.endpoint_ptr() != cap_b.endpoint_ptr() {
                    return false;
                }
                let badge = cap_a.endpoint_badge();
                badge == 0 || (badge == cap_b.endpoint_badge() && !b_first_badged)
            }
            CapTag::Notification => {
                if tag_b != CapTag::Notification
                    || cap_a.notification_ptr() != cap_b.notification_ptr()
                {
                    return false;
                }
                let badge = cap_a.notification_badge();
                badge == 0 || (badge == cap_b.notification_badge() && !b_first_badged)
            }
            CapTag::CNode => {
                tag_b == CapTag::CNode
                    && cap_a.cnode_ptr() == cap_b.cnode_ptr()
                    && cap_a.cnode_radix() == cap_b.cnode_radix()
            }
            CapTag::Thread => tag_b == CapTag::Thread && cap_a.thread_ptr() == cap_b.thread_ptr(),
            CapTag::Reply => {
                tag_b == CapTag::Reply && cap_a.reply_tcb_ptr() == cap_b.reply_tcb_ptr()
            }
            CapTag::IrqControl => tag_b == CapTag::IrqControl || tag_b == CapTag::IrqHandler,
            CapTag::IrqHandler => {
                tag_b == CapTag::IrqHandler && cap_a.irq_handler_irq() == cap_b.irq_handler_irq()
            }
            CapTag::Frame => {
                if tag_b != CapTag::Frame {
                    return false;
                }
                let (Some(a_bits), Some(b_bits)) = (
                    frame_size_bits(cap_a.frame_size()),
                    frame_size_bits(cap_b.frame_size()),
                ) else {
                    return false;
                };
                let a_base = cap_a.frame_base_ptr();
                let b_base = cap_b.frame_base_ptr();
                let a_top = region_top(a_base, a_bits);
                let b_top = region_top(b_base, b_bits);
                a_base <= b_base && b_top <= a_top && b_base <= b_top
            }
            CapTag::PageTable => {
                tag_b == CapTag::PageTable
                    && cap_a.page_table_base_ptr() == cap_b.page_table_base_ptr()
            }
            CapTag::Domain => tag_b == CapTag::Domain,
            CapTag::AsidControl => tag_b == CapTag::AsidControl,
            CapTag::AsidPool => {
                tag_b == CapTag::AsidPool && cap_a.asid_pool_ptr() == cap_b.asid_pool_ptr()
            }
            _ => false,
        }
    }
}

/// A capability table: `2^radix` consecutive slots.
pub type CNode = ObjArray<Cte>;

/// View a contiguous kernel-allocated memory region as a CNode of `radix`
/// bits.
///
/// # Safety
/// `base` must be suitably aligned and name at least `cnode_bytes(radix)`
/// bytes of memory the kernel owns.
pub unsafe fn cnode_at(base: u64, radix: usize) -> Option<CNode> {
    debug_assert!(base as usize & 0xF == 0, "CNode must be 16-byte aligned");
    // SAFETY: forwarded to the caller.
    let first = unsafe { ObjRef::from_kva(base) }?;
    // SAFETY: the caller promised the region holds this many slots.
    Some(unsafe { ObjArray::new(first, 1usize << radix) })
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

/// Mirror C kernel `isCapRevocable(newCap, srcCap)` for the cap types this
/// kernel currently implements.
pub(crate) fn is_cap_revocable(new_cap: Cap, src_cap: Cap) -> bool {
    match new_cap.tag() {
        Some(CapTag::Frame) | Some(CapTag::PageTable) | Some(CapTag::AsidPool) => false,
        Some(CapTag::Untyped) => true,
        Some(CapTag::Endpoint) => new_cap.endpoint_badge() != src_cap.endpoint_badge(),
        Some(CapTag::Notification) => new_cap.notification_badge() != src_cap.notification_badge(),
        Some(CapTag::IrqHandler) => src_cap.tag() == Some(CapTag::IrqControl),
        _ => false,
    }
}

fn region_top(base: u64, size_bits: u64) -> u64 {
    base.wrapping_add(region_mask(size_bits))
}

fn region_mask(size_bits: u64) -> u64 {
    if size_bits >= u64::BITS as u64 {
        u64::MAX
    } else {
        (1u64 << size_bits) - 1
    }
}

/// Mirror the `cap_get_capIsPhysical` + `cap_get_capPtr` +
/// `cap_get_capSizeBits` path used by seL4 `sameRegionAs(Untyped, child)`.
fn physical_cap_region(cap: Cap) -> Option<(u64, u64)> {
    match cap.tag()? {
        CapTag::Untyped => Some((cap.untyped_ptr(), cap.untyped_block_size_bits())),
        CapTag::Endpoint => Some((cap.endpoint_ptr(), SEL4_ENDPOINT_BITS as u64)),
        CapTag::Notification => Some((cap.notification_ptr(), SEL4_NOTIFICATION_BITS as u64)),
        CapTag::CNode => Some((cap.cnode_ptr(), cap.cnode_radix() + SEL4_SLOT_BITS as u64)),
        CapTag::Thread => Some((cap.thread_ptr(), SEL4_TCB_BITS as u64)),
        CapTag::Zombie => Some((cap.zombie_ptr(), zombie_region_size_bits(cap))),
        CapTag::Frame => Some((cap.frame_base_ptr(), frame_size_bits(cap.frame_size())?)),
        CapTag::PageTable => Some((cap.page_table_base_ptr(), SEL4_PAGE_TABLE_BITS as u64)),
        CapTag::AsidPool => Some((cap.asid_pool_ptr(), SEL4_ASID_POOL_BITS as u64)),
        _ => None,
    }
}

fn zombie_region_size_bits(cap: Cap) -> u64 {
    if cap.zombie_is_tcb() {
        SEL4_TCB_BITS as u64
    } else {
        cap.zombie_bits() + SEL4_SLOT_BITS as u64
    }
}

fn frame_size_bits(size: u64) -> Option<u64> {
    match size {
        0 => Some(12),
        1 => Some(21),
        2 => Some(30),
        _ => None,
    }
}

/// Empty every slot of a freshly allocated CNode.
pub fn clear_cnode(cnode: CNode) {
    cnode.with_slice_mut(|slots| slots.fill(Cte::null()));
}

/// Install `cap` (with empty MDB linkage) at slot `index`. Panics if the slot
/// is non-empty.
///
/// Mirrors `write_slot` from `kernel/src/object/cnode.c`: all initial
/// boot-time caps have `revocable = true` and `first_badged = true` so that
/// the CDT walker treats them as legitimate roots, and so derived caps
/// register as proper children via `is_mdb_parent_of`.
pub fn install_initial_cap(cnode: CNode, index: usize, cap: Cap) {
    let slot = cnode
        .get(index)
        .expect("initial cap slot index out of range");
    assert!(slot.cap().is_null(), "slot {index} is already populated");
    let mut mdb = MdbNode::NULL;
    mdb.set_revocable(true);
    mdb.set_first_badged(true);
    slot.with_mut(|cte| {
        cte.cap = cap;
        cte.mdb = mdb;
    });
}
