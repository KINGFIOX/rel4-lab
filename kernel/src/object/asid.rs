//! Minimal ASID → root-page-table-PA table.
//!
//! The real seL4 kernel uses a 16-bit ASID stored on every Frame /
//! PageTable cap so that a cap-only `Page_Unmap` can find the right
//! VSpace without help from the caller. We don't need fancy ASID
//! recycling yet: each `Page_Map` that targets a fresh root PT gets
//! assigned a slot in this table, and the slot index goes into the
//! cap's `capFMappedASID` field.
//!
//! ASID 0 is reserved as "unassigned" — Page_Unmap on a cap whose
//! `mapped_address` is zero is a no-op, so we never read slot 0.

use core::sync::atomic::{AtomicBool, AtomicU16, AtomicU64, Ordering};

/// Total ASID slots. Sized to handle the rootserver plus one VSpace per
/// spawned test process (sel4test rarely keeps more than two alive at
/// once). Bumped to 64 to leave headroom; each entry is just 8 bytes.
const ASID_TABLE_LEN: usize = 1 << 16;
const ASID_POOL_COUNT: usize = 1 << 7;
const ASID_POOL_SIZE: u16 = 1 << 9;

struct AsidEntry {
    /// Kernel VA of the Sv39 root PT. 0 means "free".
    root_pt_kva: AtomicU64,
}

static ASID_TABLE: [AsidEntry; ASID_TABLE_LEN] = {
    const E: AsidEntry = AsidEntry {
        root_pt_kva: AtomicU64::new(0),
    };
    [E; ASID_TABLE_LEN]
};

/// Next-free hint for the fallback allocator used by early/lazy mappings.
static NEXT_FREE: AtomicU16 = AtomicU16::new(1);
static POOL_ACTIVE: [AtomicBool; ASID_POOL_COUNT] =
    [const { AtomicBool::new(false) }; ASID_POOL_COUNT];
static POOL_PTR: [AtomicU64; ASID_POOL_COUNT] = [const { AtomicU64::new(0) }; ASID_POOL_COUNT];

pub fn init_root(root_pt_kva: u64) {
    register(1, root_pt_kva);
    POOL_ACTIVE[0].store(true, Ordering::Release);
    POOL_PTR[0].store(0, Ordering::Release);
    NEXT_FREE.store(2, Ordering::Release);
}

pub fn alloc_pool_base(pool_kva: u64) -> Option<u16> {
    for pool in 0..ASID_POOL_COUNT {
        if POOL_ACTIVE[pool]
            .compare_exchange(false, true, Ordering::AcqRel, Ordering::Acquire)
            .is_ok()
        {
            POOL_PTR[pool].store(pool_kva, Ordering::Release);
            clear_pool_entries(pool);
            return Some((pool as u16).wrapping_mul(ASID_POOL_SIZE));
        }
    }
    None
}

pub fn assign_from_pool(base: u16, pool_kva: u64, root_pt_kva: u64) -> Option<u16> {
    let pool = (base / ASID_POOL_SIZE) as usize;
    if pool >= ASID_POOL_COUNT || root_pt_kva == 0 {
        return None;
    }
    if !POOL_ACTIVE[pool].load(Ordering::Acquire) {
        return None;
    }
    if POOL_PTR[pool].load(Ordering::Acquire) != pool_kva {
        return None;
    }
    for off in 0..ASID_POOL_SIZE {
        let asid = base.wrapping_add(off);
        if asid == 0 {
            continue;
        }
        let slot = &ASID_TABLE[asid as usize].root_pt_kva;
        if slot
            .compare_exchange(0, root_pt_kva, Ordering::AcqRel, Ordering::Acquire)
            .is_ok()
        {
            return Some(asid);
        }
    }
    None
}

pub fn delete_pool(base: u16, pool_kva: u64) {
    let pool = (base / ASID_POOL_SIZE) as usize;
    if pool >= ASID_POOL_COUNT {
        return;
    }
    if !POOL_ACTIVE[pool].load(Ordering::Acquire) {
        return;
    }
    if POOL_PTR[pool].load(Ordering::Acquire) != pool_kva {
        return;
    }
    clear_pool_entries(pool);
    POOL_PTR[pool].store(0, Ordering::Release);
    POOL_ACTIVE[pool].store(false, Ordering::Release);
}

pub fn delete(asid: u16, root_pt_kva: u64) {
    if asid == 0 || (asid as usize) >= ASID_TABLE_LEN || root_pt_kva == 0 {
        return;
    }
    let slot = &ASID_TABLE[asid as usize].root_pt_kva;
    let _ = slot.compare_exchange(root_pt_kva, 0, Ordering::AcqRel, Ordering::Acquire);
}

fn clear_pool_entries(pool: usize) {
    let start = pool * ASID_POOL_SIZE as usize;
    let end = start + ASID_POOL_SIZE as usize;
    for asid in start..end {
        ASID_TABLE[asid].root_pt_kva.store(0, Ordering::Release);
    }
}

pub fn register(asid: u16, root_pt_kva: u64) {
    if asid == 0 || root_pt_kva == 0 {
        return;
    }
    ASID_TABLE[asid as usize]
        .root_pt_kva
        .store(root_pt_kva, Ordering::Release);
}

/// Look up an existing ASID for `root_pt_kva`, or allocate a fresh one.
/// Returns `0` if the table is full (caller must treat that as "no
/// stored mapping" and fall back to refusing the operation).
pub fn assign(root_pt_kva: u64) -> u16 {
    if root_pt_kva == 0 {
        return 0;
    }
    // Linear scan first; cheap because the table is tiny.
    for (i, slot) in ASID_TABLE.iter().enumerate().skip(1) {
        if slot.root_pt_kva.load(Ordering::Acquire) == root_pt_kva {
            return i as u16;
        }
    }
    // Allocate a new entry. Single-thread for now so no real race.
    loop {
        let i = NEXT_FREE.fetch_add(1, Ordering::AcqRel);
        if (i as usize) >= ASID_TABLE_LEN {
            return 0;
        }
        let cur = ASID_TABLE[i as usize].root_pt_kva.load(Ordering::Acquire);
        if cur == 0 {
            ASID_TABLE[i as usize]
                .root_pt_kva
                .store(root_pt_kva, Ordering::Release);
            return i;
        }
    }
}

/// Resolve an ASID to its root-PT KVA, or `0` if the slot is unused.
pub fn lookup(asid: u16) -> u64 {
    if (asid as usize) >= ASID_TABLE_LEN || asid == 0 {
        return 0;
    }
    ASID_TABLE[asid as usize]
        .root_pt_kva
        .load(Ordering::Acquire)
}
