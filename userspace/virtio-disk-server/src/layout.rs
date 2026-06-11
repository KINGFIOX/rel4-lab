use xv6_abi::{
    FS_BLOCK_SIZE, XV6_DISK_COMPLETION_ENTRY_WORDS, XV6_DISK_MAX_IN_FLIGHT,
    XV6_DISK_SHARED_BUFFER_VADDR, XV6_VIRTIO_DMA_VADDR,
};

pub const DESC_OFF: u64 = 0x000;
pub const AVAIL_OFF: u64 = 0x100;
pub const USED_OFF: u64 = 0x200;
pub const REQUEST_AREA_OFF: u64 = 0x300;
pub const REQUEST_STRIDE: u64 = 0x600;
pub const REQ_REL_OFF: u64 = 0x000;
pub const DATA_REL_OFF: u64 = 0x100;
pub const STATUS_REL_OFF: u64 = 0x500;
pub const DESCS_PER_REQUEST: u16 = 3;
pub const COMPLETION_WRITE_IDX_OFF: u64 = 0;
pub const COMPLETION_READ_IDX_OFF: u64 = 8;
pub const COMPLETION_ENTRIES_OFF: u64 = 16;
pub const COMPLETION_ENTRY_STRIDE: u64 = (XV6_DISK_COMPLETION_ENTRY_WORDS as u64) * 8;

pub fn desc_head(request_slot: usize) -> u16 {
    (request_slot as u16) * DESCS_PER_REQUEST
}

pub fn request_slot_from_head(head: u16) -> Option<usize> {
    if head % DESCS_PER_REQUEST != 0 {
        return None;
    }
    let slot = (head / DESCS_PER_REQUEST) as usize;
    if slot < XV6_DISK_MAX_IN_FLIGHT {
        Some(slot)
    } else {
        None
    }
}

pub fn req_off(request_slot: usize) -> u64 {
    REQUEST_AREA_OFF + request_slot as u64 * REQUEST_STRIDE + REQ_REL_OFF
}

pub fn data_off(request_slot: usize) -> u64 {
    REQUEST_AREA_OFF + request_slot as u64 * REQUEST_STRIDE + DATA_REL_OFF
}

pub fn status_off(request_slot: usize) -> u64 {
    REQUEST_AREA_OFF + request_slot as u64 * REQUEST_STRIDE + STATUS_REL_OFF
}

pub fn shared_buffer_va(shared_slot: u64) -> usize {
    (XV6_DISK_SHARED_BUFFER_VADDR + shared_slot * FS_BLOCK_SIZE as u64) as usize
}

pub fn dma_va(offset: u64) -> usize {
    (XV6_VIRTIO_DMA_VADDR + offset) as usize
}
