//! CNode (capability node) — a flat array of `cte_t` slots.
//!
//! A CNode of radix `r` has `2^r` slots, each `sizeof(cte_t) = 32` bytes.
//! Slot 0 is the null cap by convention. The root CNode for our build is
//! `radix = CONFIG_ROOT_CNODE_SIZE_BITS = 13`, i.e. 8192 slots = 256 KiB.

#![allow(dead_code)]

use core::ptr;

use crate::abi::constants::{
    SEL4_ASID_POOL_BITS, SEL4_ENDPOINT_BITS, SEL4_MIN_UNTYPED_BITS, SEL4_NOTIFICATION_BITS,
    SEL4_PAGE_TABLE_BITS, SEL4_REPLY_BITS, SEL4_SLOT_BITS, SEL4_TCB_BITS,
};
use crate::kernel::smp::BklObjectGuard;
use crate::object::cap::{Cap, CapTag};
use crate::object::mdb::MdbNode;

pub(crate) type CspaceLockGuard = BklObjectGuard;

/// Marker guard for CSpace/CDT mutation under the seL4-style big kernel lock.
#[inline]
pub(crate) fn lock_cspace() -> CspaceLockGuard {
    BklObjectGuard::new()
}

#[inline]
pub(crate) fn cap_snapshot(slot: *const Cte) -> Cap {
    let _guard = lock_cspace();
    unsafe {
        if slot.is_null() {
            Cap::null()
        } else {
            (*slot).cap
        }
    }
}

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
pub unsafe fn with_cnode_at<R>(base: *mut u8, radix: usize, op: impl FnOnce(&mut [Cte]) -> R) -> R {
    crate::kernel::smp::debug_assert_kernel_lock_held();
    debug_assert!((base as usize) & 0xF == 0, "CNode must be 16-byte aligned");
    let len = 1usize << radix;
    let slots = unsafe { core::slice::from_raw_parts_mut(base as *mut Cte, len) };
    op(slots)
}

/// Install `cap` (with empty MDB linkage) at slot `i` of `cnode`. Panics
/// if the slot is non-empty.
///
/// Mirrors `write_slot` from `kernel/src/object/cnode.c`: all initial
/// boot-time caps have `revocable = true` and `first_badged = true` so
/// that the CDT walker treats them as legitimate roots (and so derived
/// caps register as proper children via `is_mdb_parent_of`).
pub fn install_initial_cap(cnode: &mut [Cte], i: usize, cap: Cap) {
    crate::kernel::smp::debug_assert_kernel_lock_held();
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
pub unsafe fn mdb_insert_after_locked(
    _guard: &CspaceLockGuard,
    parent: *mut Cte,
    new_cte: *mut Cte,
) {
    debug_assert!(!parent.is_null() && !new_cte.is_null() && parent != new_cte);
    if parent.is_null() || new_cte.is_null() {
        panic!("mdbInsertAfter expects valid slots");
    }
    if parent == new_cte {
        panic!("mdbInsertAfter source and destination must differ");
    }
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

/// Mirror seL4 `insertNewCap`: publish a freshly-created object cap as a
/// revocable child immediately after `parent` in the MDB.
pub unsafe fn insert_new_cap_locked(
    _guard: &CspaceLockGuard,
    parent: *mut Cte,
    slot: *mut Cte,
    cap: Cap,
) {
    debug_assert!(!parent.is_null() && !slot.is_null() && parent != slot);
    if parent.is_null() || slot.is_null() {
        panic!("insertNewCap expects valid slots");
    }
    if parent == slot {
        panic!("insertNewCap parent and destination must differ");
    }

    unsafe {
        debug_assert!((*slot).cap.is_null());
        debug_assert!((*slot).mdb.prev() == 0);
        debug_assert!((*slot).mdb.next() == 0);
        if !(*slot).cap.is_null() {
            panic!("insertNewCap to non-empty destination");
        }
        if (*slot).mdb.prev() != 0 || (*slot).mdb.next() != 0 {
            panic!("insertNewCap destination MDB entry must be empty");
        }

        let next = (*parent).mdb.next();
        (*slot).cap = cap;
        (*slot).mdb = MdbNode::new(parent as u64, next, true, true);
        if next != 0 {
            let n = next as *mut Cte;
            (*n).mdb.set_prev(slot as u64);
        }
        (*parent).mdb.set_next(slot as u64);
    }
}

/// Mirror seL4 `cteInsert`: install `new_cap` into an empty destination
/// slot and link it immediately after `src_slot` in the MDB.
///
/// The caller is responsible for deriving/masking `new_cap` and validating
/// source/destination lookup rights before insertion.
pub unsafe fn cte_insert_locked(
    _cspace_guard: &CspaceLockGuard,
    new_cap: Cap,
    src_slot: *mut Cte,
    dest_slot: *mut Cte,
) {
    debug_assert!(!src_slot.is_null());
    debug_assert!(!dest_slot.is_null());
    debug_assert!(src_slot != dest_slot);
    if src_slot.is_null() || dest_slot.is_null() {
        panic!("cteInsert expects valid slots");
    }
    if src_slot == dest_slot {
        panic!("cteInsert source and destination must differ");
    }

    unsafe {
        let src_mdb = (*src_slot).mdb;
        let src_cap = (*src_slot).cap;
        let new_revocable = is_cap_revocable(new_cap, src_cap);
        let new_mdb = MdbNode::new(
            src_slot as u64,
            src_mdb.next(),
            new_revocable,
            new_revocable,
        );

        debug_assert!((*dest_slot).cap.is_null());
        debug_assert!((*dest_slot).mdb.prev() == 0);
        debug_assert!((*dest_slot).mdb.next() == 0);
        if !(*dest_slot).cap.is_null() {
            panic!("cteInsert to non-empty destination");
        }
        if (*dest_slot).mdb.prev() != 0 || (*dest_slot).mdb.next() != 0 {
            panic!("cteInsert destination MDB entry must be empty");
        }

        set_untyped_cap_as_full(src_cap, new_cap, src_slot);
        (*dest_slot).cap = new_cap;
        (*dest_slot).mdb = new_mdb;
        (*src_slot).mdb.set_next(dest_slot as u64);
        if src_mdb.next() != 0 {
            let next = src_mdb.next() as *mut Cte;
            (*next).mdb.set_prev(dest_slot as u64);
        }
    }
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

#[inline]
unsafe fn set_untyped_cap_as_full(src_cap: Cap, new_cap: Cap, src_slot: *mut Cte) {
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
    unsafe {
        (*src_slot).cap.set_untyped_free_index(free_index);
    }
}

/// Unlink `cte` from its CDT siblings. Leaves the slot otherwise intact.
pub unsafe fn mdb_unlink_locked(_guard: &CspaceLockGuard, cte: *mut Cte) {
    debug_assert!(!cte.is_null());
    if cte.is_null() {
        panic!("mdbUnlink expects a valid slot");
    }
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
/// counts as a *child* of `cte` only when the direct `mdbNext` successor
/// satisfies seL4 `isMDBParentOf(cte, next)`: `cte` is revocable AND the
/// two caps refer to the same region (e.g. same frame, same EP, or the
/// child's range is contained within `cte`'s untyped).
pub unsafe fn mdb_has_children_locked(_guard: &CspaceLockGuard, cte: *mut Cte) -> bool {
    debug_assert!(!cte.is_null());
    if cte.is_null() {
        panic!("mdbHasChildren expects a valid slot");
    }
    let next_raw = unsafe { (*cte).mdb.next() };
    if next_raw == 0 {
        return false;
    }
    let next = next_raw as *mut Cte;
    unsafe { is_mdb_parent_of(cte, next) }
}

/// Mirrors C kernel `isMDBParentOf(cte_a, cte_b)`: parent must be
/// revocable, the two caps must overlap on the same region, and
/// badged Endpoint/Notification caps only parent same-badge descendants
/// that are not the first badged cap in a branch.
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
    let b_first_badged = unsafe { (*b).mdb.first_badged() };

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
            if tag_b != CapTag::Notification || cap_a.notification_ptr() != cap_b.notification_ptr()
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
            tag_b == CapTag::Reply && cap_a.reply_object_ptr() == cap_b.reply_object_ptr()
        }
        CapTag::IrqControl => tag_b == CapTag::IrqControl || tag_b == CapTag::IrqHandler,
        CapTag::IrqHandler => {
            tag_b == CapTag::IrqHandler && cap_a.irq_handler_irq() == cap_b.irq_handler_irq()
        }
        CapTag::Frame => {
            if tag_b != CapTag::Frame {
                return false;
            }
            let Some(a_bits) = frame_size_bits(cap_a.frame_size()) else {
                return false;
            };
            let Some(b_bits) = frame_size_bits(cap_b.frame_size()) else {
                return false;
            };
            let a_base = cap_a.frame_base_ptr();
            let b_base = cap_b.frame_base_ptr();
            let a_top = region_top(a_base, a_bits);
            let b_top = region_top(b_base, b_bits);
            a_base <= b_base && b_top <= a_top && b_base <= b_top
        }
        CapTag::PageTable => {
            tag_b == CapTag::PageTable && cap_a.page_table_base_ptr() == cap_b.page_table_base_ptr()
        }
        CapTag::Domain => tag_b == CapTag::Domain,
        CapTag::AsidControl => tag_b == CapTag::AsidControl,
        CapTag::AsidPool => {
            tag_b == CapTag::AsidPool && cap_a.asid_pool_ptr() == cap_b.asid_pool_ptr()
        }
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
        CapTag::Reply => Some((cap.reply_object_ptr(), SEL4_REPLY_BITS as u64)),
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

/// Zero a freshly allocated CNode.
pub unsafe fn zero_cnode(base: *mut u8, radix: usize) {
    crate::kernel::smp::debug_assert_kernel_lock_held();
    unsafe { ptr::write_bytes(base, 0, cnode_bytes(radix)) };
}
