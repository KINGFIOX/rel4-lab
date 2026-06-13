use core::cell::UnsafeCell;

use crate::consts::{
    CHILD_SCHED_BUDGET_US, LABEL_CNODE_COPY, LABEL_CNODE_DELETE, LABEL_CNODE_MINT,
    LABEL_CNODE_REVOKE, LABEL_SCHED_CONTROL_CONFIGURE_FLAGS, LABEL_UNTYPED_RETYPE, MAX_PROCS,
    MAX_RECYCLED_SLOTS, OBJ_UNTYPED, PROCESS_UNTYPED_BITS, PROCESS_UNTYPED_PARENT_BITS, ROOT_CNODE,
    ROOT_CNODE_DEPTH, XV6_DEVICE_MMIO_BASE, XV6_DEVICE_MMIO_SIZE,
};
use crate::types::BootInfo;
use crate::util::{halt_loop, warn};
use sel4_user::call_checked;

pub(crate) struct Allocator {
    next_slot: u64,
    empty_end: u64,
    untyped_slot: u64,
    device_untyped_slot: u64,
    device_cursor_pa: u64,
    device_top_pa: u64,
    sched_control: u64,
    process_untyped_slots: [u64; MAX_PROCS],
    recycled_len: usize,
}

struct RecycledSlots {
    slots: UnsafeCell<[u64; MAX_RECYCLED_SLOTS]>,
}

// xv6-host allocates and recycles root CSpace slots from the single rootserver
// loop; the allocator's `recycled_len` serializes access to this backing store.
unsafe impl Sync for RecycledSlots {}

impl RecycledSlots {
    const fn new() -> Self {
        Self {
            slots: UnsafeCell::new([0; MAX_RECYCLED_SLOTS]),
        }
    }

    fn get(&self, index: usize) -> u64 {
        unsafe { (&*self.slots.get())[index] }
    }

    fn set(&self, index: usize, slot: u64) {
        unsafe {
            (&mut *self.slots.get())[index] = slot;
        }
    }
}

static RECYCLED_SLOTS: RecycledSlots = RecycledSlots::new();

impl Allocator {
    pub(crate) fn new(bi: &BootInfo) -> Self {
        let mut selected = 0;
        let mut selected_bits = 0u8;
        let mut process_parent = 0;
        let mut process_parent_bits = 0u8;
        let mut device_untyped_slot = 0;
        let mut device_cursor_pa = 0;
        let mut device_top_pa = 0;
        let start = bi.untyped.start as usize;
        let end = bi.untyped.end as usize;
        let mut slot = bi.untyped.start;
        for i in start..end {
            let desc = bi.untyped_list[i - start];
            if desc.is_device == 0 && desc.size_bits >= 24 {
                if desc.size_bits > selected_bits {
                    selected = slot;
                    selected_bits = desc.size_bits;
                }
                if desc.size_bits >= PROCESS_UNTYPED_PARENT_BITS
                    && desc.size_bits > process_parent_bits
                {
                    process_parent = slot;
                    process_parent_bits = desc.size_bits;
                }
            }
            if desc.is_device != 0 {
                let top = desc.paddr.saturating_add(1u64 << desc.size_bits);
                if desc.paddr <= XV6_DEVICE_MMIO_BASE
                    && top >= XV6_DEVICE_MMIO_BASE.saturating_add(XV6_DEVICE_MMIO_SIZE)
                {
                    device_untyped_slot = slot;
                    device_cursor_pa = desc.paddr;
                    device_top_pa = top;
                }
            }
            slot += 1;
        }
        if selected == 0 {
            warn!("xv6-host: no usable untyped");
            halt_loop();
        }
        if process_parent == 0 {
            warn!("xv6-host: no process untyped parent");
            halt_loop();
        }
        if device_untyped_slot == 0 {
            warn!(
                "xv6-host: no device MMIO untyped for pa={:#x}",
                XV6_DEVICE_MMIO_BASE
            );
            halt_loop();
        }
        if bi.schedcontrol.start == bi.schedcontrol.end {
            warn!("xv6-host: no schedcontrol cap");
            halt_loop();
        }
        let mut alloc = Self {
            next_slot: bi.empty.start,
            empty_end: bi.empty.end,
            untyped_slot: selected,
            device_untyped_slot,
            device_cursor_pa,
            device_top_pa,
            sched_control: bi.schedcontrol.start,
            process_untyped_slots: [0; MAX_PROCS],
            recycled_len: 0,
        };
        let mut i = 0;
        while i < MAX_PROCS {
            alloc.process_untyped_slots[i] =
                alloc.retype_one_from(process_parent, OBJ_UNTYPED, PROCESS_UNTYPED_BITS);
            i += 1;
        }
        alloc
    }

    pub(crate) fn alloc_slot(&mut self) -> u64 {
        if self.recycled_len != 0 {
            self.recycled_len -= 1;
            return RECYCLED_SLOTS.get(self.recycled_len);
        }
        if self.next_slot >= self.empty_end {
            warn!("xv6-host: out of CSpace slots");
            halt_loop();
        }
        let slot = self.next_slot;
        self.next_slot += 1;
        slot
    }

    pub(crate) fn slots_available(&self) -> usize {
        self.empty_end.saturating_sub(self.next_slot) as usize + self.recycled_len
    }

    pub(crate) fn retype_one(&mut self, ty: u64, user_size: u64) -> u64 {
        self.retype_one_from(self.untyped_slot, ty, user_size)
    }

    pub(crate) fn retype_one_from(&mut self, untyped_slot: u64, ty: u64, user_size: u64) -> u64 {
        let slot = self.alloc_slot();
        let mrs = [ty, user_size, 0, 0, slot, 1];
        call_checked(untyped_slot, LABEL_UNTYPED_RETYPE, &[ROOT_CNODE], &mrs);
        slot
    }

    pub(crate) fn configure_sched_context(&mut self, sched_context: u64, badge: u64) {
        call_checked(
            self.sched_control,
            LABEL_SCHED_CONTROL_CONFIGURE_FLAGS,
            &[sched_context],
            &[CHILD_SCHED_BUDGET_US, CHILD_SCHED_BUDGET_US, 0, badge, 0],
        );
    }

    pub(crate) fn retype_device_4k_at(&mut self, paddr: u64) -> u64 {
        if paddr & (crate::consts::PAGE_SIZE - 1) != 0
            || paddr < self.device_cursor_pa
            || paddr.saturating_add(crate::consts::PAGE_SIZE) > self.device_top_pa
        {
            warn!("xv6-host: invalid device frame request pa={:#x}", paddr);
            halt_loop();
        }
        while self.device_cursor_pa < paddr {
            let remaining = paddr - self.device_cursor_pa;
            let size_bits = largest_aligned_chunk_bits(self.device_cursor_pa, remaining);
            let _ = self.retype_one_from(self.device_untyped_slot, OBJ_UNTYPED, size_bits as u64);
            self.device_cursor_pa += 1u64 << size_bits;
        }
        let frame = self.retype_one_from(self.device_untyped_slot, crate::consts::OBJ_4K, 0);
        self.device_cursor_pa += crate::consts::PAGE_SIZE;
        frame
    }

    pub(crate) fn process_untyped(&self, proc_slot: usize) -> u64 {
        if proc_slot >= MAX_PROCS || self.process_untyped_slots[proc_slot] == 0 {
            warn!("xv6-host: invalid process untyped slot");
            halt_loop();
        }
        self.process_untyped_slots[proc_slot]
    }

    pub(crate) fn copy_cap(&mut self, src_slot: u64, rights: u64) -> u64 {
        let dst = self.alloc_slot();
        let mrs = [dst, ROOT_CNODE_DEPTH, src_slot, ROOT_CNODE_DEPTH, rights];
        call_checked(ROOT_CNODE, LABEL_CNODE_COPY, &[ROOT_CNODE], &mrs);
        dst
    }

    pub(crate) fn mint_cap(&mut self, src_slot: u64, rights: u64, badge: u64) -> u64 {
        let dst = self.alloc_slot();
        let mrs = [
            dst,
            ROOT_CNODE_DEPTH,
            src_slot,
            ROOT_CNODE_DEPTH,
            rights,
            badge,
        ];
        call_checked(ROOT_CNODE, LABEL_CNODE_MINT, &[ROOT_CNODE], &mrs);
        dst
    }

    pub(crate) fn delete_cap_slot(&mut self, slot: u64) {
        if slot == 0 {
            return;
        }
        call_checked(
            ROOT_CNODE,
            LABEL_CNODE_DELETE,
            &[],
            &[slot, ROOT_CNODE_DEPTH],
        );
        if self.recycled_len < MAX_RECYCLED_SLOTS {
            RECYCLED_SLOTS.set(self.recycled_len, slot);
            self.recycled_len += 1;
        }
    }

    pub(crate) fn revoke_cap_slot(&mut self, slot: u64) {
        if slot == 0 {
            return;
        }
        call_checked(
            ROOT_CNODE,
            LABEL_CNODE_REVOKE,
            &[],
            &[slot, ROOT_CNODE_DEPTH],
        );
    }
}

fn largest_aligned_chunk_bits(cursor: u64, remaining: u64) -> u8 {
    let max_by_size = 63 - remaining.leading_zeros() as u8;
    let max_by_align = if cursor == 0 {
        63
    } else {
        cursor.trailing_zeros() as u8
    };
    let bits = core::cmp::min(max_by_size, max_by_align);
    if bits < 12 {
        warn!("xv6-host: device untyped skip lost page alignment");
        halt_loop();
    }
    bits
}
