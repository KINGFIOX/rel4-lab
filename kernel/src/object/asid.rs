//! Minimal ASID → root-page-table-PA table.
//!
//! The real seL4 kernel uses a 16-bit ASID stored on every Frame /
//! PageTable cap so that a cap-only `Page_Unmap` can find the right
//! VSpace without help from the caller. `ASIDPool_Assign` allocates
//! within user-visible pool ranges; `Page_Map` and context switch paths
//! require the destination VSpace cap to already carry such an ASID.
//!
//! ASID 0 is reserved as "unassigned" — Page_Unmap on a cap whose
//! `capFMappedASID` is zero is a no-op, so we never read slot 0.

use crate::kernel::smp::BklCell;

/// Total ASID slots in seL4's 16-bit ASID namespace.
const ASID_TABLE_LEN: usize = 1 << 16;
const ASID_POOL_COUNT: usize = 1 << 7;
pub const ASID_POOL_ENTRY_COUNT: usize = 1 << 9;
const ASID_POOL_SIZE: u16 = 1 << 9;

#[derive(Copy, Clone)]
struct AsidEntry {
    /// Kernel VA of the Sv39 root PT. 0 means "free".
    root_pt_kva: u64,
}

const EMPTY_ASID_ENTRY: AsidEntry = AsidEntry { root_pt_kva: 0 };

struct AsidState {
    table: [AsidEntry; ASID_TABLE_LEN],
    pool_active: [bool; ASID_POOL_COUNT],
    pool_ptr: [u64; ASID_POOL_COUNT],
}

impl AsidState {
    const fn new() -> Self {
        Self {
            table: [EMPTY_ASID_ENTRY; ASID_TABLE_LEN],
            pool_active: [false; ASID_POOL_COUNT],
            pool_ptr: [0; ASID_POOL_COUNT],
        }
    }

    fn clear_pool_entries(&mut self, pool: usize) {
        let start = pool * ASID_POOL_SIZE as usize;
        let end = start + ASID_POOL_SIZE as usize;
        for asid in start..end {
            self.table[asid].root_pt_kva = 0;
        }
    }

    fn register(&mut self, asid: u16, root_pt_kva: u64) {
        if asid == 0 || root_pt_kva == 0 {
            return;
        }
        self.table[asid as usize].root_pt_kva = root_pt_kva;
    }
}

static ASID_STATE: BklCell<AsidState> = BklCell::new(AsidState::new());

#[derive(Copy, Clone, Debug, Eq, PartialEq)]
pub enum AsidPoolAssignError {
    MissingPool,
    WrongPool,
    Full,
}

#[inline]
pub const fn pool_index(asid: u16) -> usize {
    (asid as usize) & (ASID_POOL_ENTRY_COUNT - 1)
}

unsafe fn set_pool_entry(pool_kva: u64, index: usize, root_pt_kva: u64) {
    if pool_kva == 0 || index >= ASID_POOL_ENTRY_COUNT {
        return;
    }
    unsafe {
        *((pool_kva as *mut u64).add(index)) = root_pt_kva;
    }
}

pub fn init_root(root_pt_kva: u64, pool_kva: u64) {
    ASID_STATE.with_mut(|state| {
        state.register(1, root_pt_kva);
        state.pool_active[0] = true;
        state.pool_ptr[0] = pool_kva;
    });
}

pub fn next_free_pool_base() -> Option<u16> {
    ASID_STATE.with_ref(|state| {
        for pool in 0..ASID_POOL_COUNT {
            if !state.pool_active[pool] {
                return Some((pool as u16).wrapping_mul(ASID_POOL_SIZE));
            }
        }
        None
    })
}

pub fn publish_pool(base: u16, pool_kva: u64) -> bool {
    ASID_STATE.with_mut(|state| {
        let pool = (base / ASID_POOL_SIZE) as usize;
        let pool_start = pool * ASID_POOL_SIZE as usize;
        if pool >= ASID_POOL_COUNT || base as usize != pool_start || pool_kva == 0 {
            return false;
        }
        if state.pool_active[pool] {
            return false;
        }
        state.clear_pool_entries(pool);
        state.pool_ptr[pool] = pool_kva;
        state.pool_active[pool] = true;
        true
    })
}

pub fn next_free_from_pool(base: u16, pool_kva: u64) -> Result<u16, AsidPoolAssignError> {
    ASID_STATE.with_ref(|state| {
        let pool = (base / ASID_POOL_SIZE) as usize;
        let pool_start = pool * ASID_POOL_SIZE as usize;
        let pool_end = pool_start + ASID_POOL_SIZE as usize;
        if pool >= ASID_POOL_COUNT || base as usize != pool_start {
            return Err(AsidPoolAssignError::MissingPool);
        }
        if !state.pool_active[pool] {
            return Err(AsidPoolAssignError::MissingPool);
        }
        if state.pool_ptr[pool] != pool_kva {
            return Err(AsidPoolAssignError::WrongPool);
        }
        for asid_idx in pool_start..pool_end {
            let asid = asid_idx as u16;
            if asid == 0 {
                continue;
            }
            if state.table[asid as usize].root_pt_kva == 0 {
                return Ok(asid);
            }
        }
        Err(AsidPoolAssignError::Full)
    })
}

pub fn publish_pool_assignment(base: u16, pool_kva: u64, asid: u16, root_pt_kva: u64) -> bool {
    ASID_STATE.with_mut(|state| {
        let pool = (base / ASID_POOL_SIZE) as usize;
        let pool_start = pool * ASID_POOL_SIZE as usize;
        let pool_end = pool_start + ASID_POOL_SIZE as usize;
        if pool >= ASID_POOL_COUNT
            || base as usize != pool_start
            || root_pt_kva == 0
            || asid == 0
            || (asid as usize) < pool_start
            || (asid as usize) >= pool_end
        {
            return false;
        }
        if !state.pool_active[pool] {
            return false;
        }
        if state.pool_ptr[pool] != pool_kva {
            return false;
        }
        let slot = &mut state.table[asid as usize].root_pt_kva;
        if *slot != 0 {
            return false;
        }
        *slot = root_pt_kva;
        unsafe { set_pool_entry(pool_kva, pool_index(asid), root_pt_kva) };
        true
    })
}

pub fn delete_pool(base: u16, pool_kva: u64) {
    let deleted = ASID_STATE.with_mut(|state| {
        let pool = (base / ASID_POOL_SIZE) as usize;
        let pool_start = pool * ASID_POOL_SIZE as usize;
        if pool >= ASID_POOL_COUNT || base as usize != pool_start {
            return false;
        }
        if !state.pool_active[pool] {
            return false;
        }
        if state.pool_ptr[pool] != pool_kva {
            return false;
        }
        state.clear_pool_entries(pool);
        state.pool_ptr[pool] = 0;
        state.pool_active[pool] = false;
        true
    });
    if deleted {
        crate::arch::current::vspace::set_current_vspace_root();
    }
}

pub fn delete(asid: u16, root_pt_kva: u64) {
    let deleted = ASID_STATE.with_mut(|state| {
        if asid == 0 || (asid as usize) >= ASID_TABLE_LEN || root_pt_kva == 0 {
            return false;
        }
        let pool = (asid / ASID_POOL_SIZE) as usize;
        let slot = &mut state.table[asid as usize].root_pt_kva;
        if *slot != root_pt_kva {
            return false;
        }
        crate::kernel::smp::sfence_vma_asid_all_harts(asid as usize);
        *slot = 0;
        if pool < ASID_POOL_COUNT && state.pool_active[pool] {
            unsafe { set_pool_entry(state.pool_ptr[pool], pool_index(asid), 0) };
        }
        true
    });
    if deleted {
        crate::arch::current::vspace::set_current_vspace_root();
    }
}

pub fn register(asid: u16, root_pt_kva: u64) {
    ASID_STATE.with_mut(|state| state.register(asid, root_pt_kva));
}

/// Resolve an ASID to its root-PT KVA, or `0` if the slot is unused.
pub fn lookup(asid: u16) -> u64 {
    ASID_STATE.with_ref(|state| {
        if (asid as usize) >= ASID_TABLE_LEN || asid == 0 {
            return 0;
        }
        state.table[asid as usize].root_pt_kva
    })
}
