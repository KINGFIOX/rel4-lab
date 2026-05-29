use crate::consts::FD_CLOSED;

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
    pub(crate) child_page: u64,
    pub(crate) alias_page: u64,
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

pub(crate) struct Child {
    pub(crate) tcb: u64,
    pub(crate) vspace: u64,
    pub(crate) fault_ep: u64,
    pub(crate) entry: u64,
    pub(crate) brk: u64,
    pub(crate) heap_mapped_end: u64,
}

pub(crate) struct IpcMessage {
    pub(crate) info: u64,
    pub(crate) mrs: [u64; 16],
}
