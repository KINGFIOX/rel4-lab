use crate::consts::{
    LABEL_CNODE_COPY, LABEL_CNODE_DELETE, LABEL_CNODE_REVOKE, LABEL_UNTYPED_RETYPE, MAX_PROCS,
    MAX_RECYCLED_SLOTS, OBJ_UNTYPED, PROCESS_UNTYPED_BITS, PROCESS_UNTYPED_PARENT_BITS, ROOT_CNODE,
    ROOT_CNODE_DEPTH,
};
use crate::sel4::call_checked;
use crate::types::BootInfo;
use crate::util::{halt_loop, log};

pub(crate) struct Allocator {
    next_slot: u64,
    empty_end: u64,
    untyped_slot: u64,
    process_untyped_slots: [u64; MAX_PROCS],
    recycled_len: usize,
}

static mut RECYCLED_SLOTS: [u64; MAX_RECYCLED_SLOTS] = [0; MAX_RECYCLED_SLOTS];

impl Allocator {
    pub(crate) fn new(bi: &BootInfo) -> Self {
        let mut selected = 0;
        let mut process_parent = 0;
        let mut process_parent_bits = 0u8;
        let start = bi.untyped.start as usize;
        let end = bi.untyped.end as usize;
        let mut slot = bi.untyped.start;
        for i in start..end {
            let desc = bi.untyped_list[i - start];
            if desc.is_device == 0 && desc.size_bits >= 24 {
                if selected == 0 {
                    selected = slot;
                }
                if desc.size_bits >= PROCESS_UNTYPED_PARENT_BITS
                    && desc.size_bits > process_parent_bits
                {
                    process_parent = slot;
                    process_parent_bits = desc.size_bits;
                }
            }
            slot += 1;
        }
        if selected == 0 {
            log("xv6-host: no usable untyped\n");
            halt_loop();
        }
        if process_parent == 0 {
            log("xv6-host: no process untyped parent\n");
            halt_loop();
        }
        let mut alloc = Self {
            next_slot: bi.empty.start,
            empty_end: bi.empty.end,
            untyped_slot: selected,
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
            return unsafe { RECYCLED_SLOTS[self.recycled_len] };
        }
        if self.next_slot >= self.empty_end {
            log("xv6-host: out of CSpace slots\n");
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

    pub(crate) fn process_untyped(&self, proc_slot: usize) -> u64 {
        if proc_slot >= MAX_PROCS || self.process_untyped_slots[proc_slot] == 0 {
            log("xv6-host: invalid process untyped slot\n");
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
            unsafe {
                RECYCLED_SLOTS[self.recycled_len] = slot;
            }
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
