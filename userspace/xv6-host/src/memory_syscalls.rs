use core::cmp::min;
use core::sync::atomic::{AtomicU64, Ordering};

use crate::allocator::Allocator;
use crate::child::{
    frame_pool_available, is_child_page_mapped, map_lazy_child_page, mapping_slots_available,
    unmap_child_range,
};
use crate::consts::*;
use crate::types::TaskStruct;
use crate::util::align_up;

static SPARSE_EAGER_RESERVED: AtomicU64 = AtomicU64::new(0);

pub(crate) fn sys_sbrk(
    alloc: &mut Allocator,
    child: &mut TaskStruct,
    increment: i64,
    mode: u64,
) -> i64 {
    let old = child.brk;
    let new_brk = if increment >= 0 {
        old.saturating_add(increment as u64)
    } else {
        let decrement = (-increment) as u64;
        if decrement > old {
            old
        } else {
            old - decrement
        }
    };
    if new_brk > CHILD_HEAP_LIMIT {
        return -1;
    }
    if mode != SBRK_EAGER && mode != SBRK_LAZY {
        return -1;
    }
    if new_brk < old {
        release_sparse_eager(child, old - new_brk);
        let new_mapped_end = align_up(new_brk);
        unmap_child_range(alloc, child.pid, new_mapped_end, align_up(old));
        child.heap_mapped_end = child.heap_mapped_end.min(new_mapped_end);
    }
    if new_brk > old {
        let target_end = align_up(new_brk);
        let first_page = if mode == SBRK_EAGER {
            align_up(old)
        } else {
            align_up(child.heap_mapped_end)
        };
        let needed = ((target_end.saturating_sub(first_page)) / PAGE_SIZE) as usize;
        if needed <= SBRK_EAGER_MAP_LIMIT {
            let mapping_available = mapping_slots_available();
            let pooled_frames = frame_pool_available();
            let slot_available = alloc.slots_available().saturating_add(pooled_frames);
            if needed > mapping_available.saturating_sub(SBRK_MAPPING_HEADROOM)
                || needed > slot_available.saturating_sub(SBRK_MAPPING_HEADROOM)
            {
                return -1;
            }
            let mut page = first_page;
            while page < target_end {
                map_lazy_child_page(alloc, child, page, true, false);
                page += PAGE_SIZE;
            }
            if mode != SBRK_EAGER || target_end > child.heap_mapped_end {
                child.heap_mapped_end = target_end;
            }
        } else if mode == SBRK_EAGER {
            let sparse_bytes = target_end.saturating_sub(first_page);
            if !reserve_sparse_eager(child, sparse_bytes) {
                return -1;
            }
        }
    }
    child.brk = new_brk;
    old as i64
}

pub(crate) fn sparse_eager_can_clone(parent: &TaskStruct) -> bool {
    SPARSE_EAGER_RESERVED
        .load(Ordering::Relaxed)
        .saturating_add(parent.sparse_reserved)
        <= SPARSE_EAGER_RESERVE_LIMIT
}

pub(crate) fn reserve_sparse_eager(child: &mut TaskStruct, bytes: u64) -> bool {
    loop {
        let reserved = SPARSE_EAGER_RESERVED.load(Ordering::Relaxed);
        let new_reserved = reserved.saturating_add(bytes);
        if new_reserved > SPARSE_EAGER_RESERVE_LIMIT {
            return false;
        }
        if SPARSE_EAGER_RESERVED
            .compare_exchange_weak(reserved, new_reserved, Ordering::Relaxed, Ordering::Relaxed)
            .is_ok()
        {
            child.sparse_reserved = child.sparse_reserved.saturating_add(bytes);
            return true;
        }
    }
}

fn release_sparse_eager(child: &mut TaskStruct, bytes: u64) {
    let n = min(child.sparse_reserved, bytes);
    child.sparse_reserved -= n;
    release_sparse_eager_reserved(n);
}

pub(crate) fn release_all_sparse_eager(child: &mut TaskStruct) {
    let n = child.sparse_reserved;
    child.sparse_reserved = 0;
    release_sparse_eager_reserved(n);
}

fn release_sparse_eager_reserved(bytes: u64) {
    let _ = SPARSE_EAGER_RESERVED.fetch_update(Ordering::Relaxed, Ordering::Relaxed, |reserved| {
        Some(reserved.saturating_sub(bytes))
    });
}

pub(crate) fn handle_lazy_page_fault(
    alloc: &mut Allocator,
    child: &mut TaskStruct,
    fault_addr: u64,
    fsr: u64,
) -> bool {
    if fault_addr < child.heap_start || fault_addr >= child.brk || fault_addr >= CHILD_HEAP_LIMIT {
        return false;
    }
    if is_child_page_mapped(child, fault_addr) {
        return false;
    }
    if fsr != 5 && fsr != 7 {
        return false;
    }
    if mapping_slots_available() <= SBRK_MAPPING_HEADROOM {
        return false;
    }
    if frame_pool_available() == 0 && alloc.slots_available() <= SBRK_MAPPING_HEADROOM {
        return false;
    }
    map_lazy_child_page(alloc, child, fault_addr, true, false);
    true
}
