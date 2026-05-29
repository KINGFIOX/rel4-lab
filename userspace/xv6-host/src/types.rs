use crate::consts::{FD_CLOSED, PROC_UNUSED};

#[repr(C)]
pub(crate) struct SlotRegion {
    pub(crate) start: u64,
    pub(crate) end: u64,
}

#[repr(C)]
#[derive(Copy, Clone)]
pub(crate) struct UntypedDesc {
    pub(crate) paddr: u64,
    pub(crate) size_bits: u8,
    pub(crate) is_device: u8,
    pub(crate) _padding: [u8; 6],
}

#[repr(C)]
pub(crate) struct BootInfo {
    pub(crate) extra_len: u64,
    pub(crate) node_id: u64,
    pub(crate) num_nodes: u64,
    pub(crate) num_io_pt_levels: u64,
    pub(crate) ipc_buffer: u64,
    pub(crate) empty: SlotRegion,
    pub(crate) shared_frames: SlotRegion,
    pub(crate) user_image_frames: SlotRegion,
    pub(crate) user_image_paging: SlotRegion,
    pub(crate) io_space_caps: SlotRegion,
    pub(crate) extra_bi_pages: SlotRegion,
    pub(crate) init_thread_cnode_size_bits: u64,
    pub(crate) init_thread_domain: u8,
    pub(crate) _pad_domain: [u8; 7],
    pub(crate) untyped: SlotRegion,
    pub(crate) untyped_list: [UntypedDesc; 230],
}

#[repr(C)]
pub(crate) struct IpcBuffer {
    pub(crate) tag: u64,
    pub(crate) msg: [u64; 120],
    pub(crate) user_data: u64,
    pub(crate) caps_or_badges: [u64; 3],
    pub(crate) receive_cnode: u64,
    pub(crate) receive_index: u64,
    pub(crate) receive_depth: u64,
}

#[derive(Copy, Clone)]
pub(crate) struct Mapping {
    pub(crate) pid: u64,
    pub(crate) child_page: u64,
    pub(crate) alias_page: u64,
    pub(crate) writable: bool,
    pub(crate) executable: bool,
}

#[derive(Copy, Clone)]
pub(crate) struct FdEntry {
    pub(crate) kind: u8,
    pub(crate) offset: usize,
    pub(crate) aux: usize,
}

impl FdEntry {
    pub(crate) const fn closed() -> Self {
        Self {
            kind: FD_CLOSED,
            offset: 0,
            aux: 0,
        }
    }
}

#[derive(Copy, Clone)]
pub(crate) struct Child {
    pub(crate) pid: u64,
    pub(crate) parent_pid: u64,
    pub(crate) state: u8,
    pub(crate) exit_status: i32,
    pub(crate) tcb: u64,
    pub(crate) vspace: u64,
    pub(crate) fault_ep: u64,
    pub(crate) entry: u64,
    pub(crate) brk: u64,
    pub(crate) heap_mapped_end: u64,
    pub(crate) wait_status_ptr: u64,
    pub(crate) wait_reply_slot: u64,
    pub(crate) wait_reply_mrs: [u64; 11],
}

impl Child {
    pub(crate) const fn empty() -> Self {
        Self {
            pid: 0,
            parent_pid: 0,
            state: PROC_UNUSED,
            exit_status: 0,
            tcb: 0,
            vspace: 0,
            fault_ep: 0,
            entry: 0,
            brk: 0,
            heap_mapped_end: 0,
            wait_status_ptr: 0,
            wait_reply_slot: 0,
            wait_reply_mrs: [0; 11],
        }
    }
}

pub(crate) struct IpcMessage {
    pub(crate) badge: u64,
    pub(crate) info: u64,
    pub(crate) mrs: [u64; 64],
}
