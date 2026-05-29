use crate::consts::{LABEL_CNODE_COPY, LABEL_UNTYPED_RETYPE, ROOT_CNODE, ROOT_CNODE_DEPTH};
use crate::sel4::call_checked;
use crate::types::BootInfo;
use crate::util::{halt_loop, log};

pub(crate) struct Allocator {
    next_slot: u64,
    empty_end: u64,
    untyped_slot: u64,
}

impl Allocator {
    pub(crate) fn new(bi: &BootInfo) -> Self {
        let mut selected = 0;
        let start = bi.untyped.start as usize;
        let end = bi.untyped.end as usize;
        let mut slot = bi.untyped.start;
        for i in start..end {
            let desc = bi.untyped_list[i - start];
            if desc.is_device == 0 && desc.size_bits >= 24 {
                selected = slot;
                break;
            }
            slot += 1;
        }
        if selected == 0 {
            log("xv6-host: no usable untyped\n");
            halt_loop();
        }
        Self {
            next_slot: bi.empty.start,
            empty_end: bi.empty.end,
            untyped_slot: selected,
        }
    }

    pub(crate) fn alloc_slot(&mut self) -> u64 {
        if self.next_slot >= self.empty_end {
            log("xv6-host: out of CSpace slots\n");
            halt_loop();
        }
        let slot = self.next_slot;
        self.next_slot += 1;
        slot
    }

    pub(crate) fn retype_one(&mut self, ty: u64, user_size: u64) -> u64 {
        let slot = self.alloc_slot();
        let mrs = [ty, user_size, 0, 0, slot, 1];
        call_checked(self.untyped_slot, LABEL_UNTYPED_RETYPE, &[ROOT_CNODE], &mrs);
        slot
    }

    pub(crate) fn copy_cap(&mut self, src_slot: u64, rights: u64) -> u64 {
        let dst = self.alloc_slot();
        let mrs = [dst, ROOT_CNODE_DEPTH, src_slot, ROOT_CNODE_DEPTH, rights];
        call_checked(ROOT_CNODE, LABEL_CNODE_COPY, &[ROOT_CNODE], &mrs);
        dst
    }
}
