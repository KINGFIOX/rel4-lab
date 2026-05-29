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

use core::sync::atomic::{AtomicU16, AtomicU64, Ordering};

/// Total ASID slots. Sized to handle the rootserver plus one VSpace per
/// spawned test process (sel4test rarely keeps more than two alive at
/// once). Bumped to 64 to leave headroom; each entry is just 8 bytes.
const ASID_TABLE_LEN: usize = 64;

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

/// Next-free hint. We never recycle in M3.
static NEXT_FREE: AtomicU16 = AtomicU16::new(1);

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
