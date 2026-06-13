compile_error!(
    "virtio-disk-server LoongArch64 requires a virtio-pci backend for QEMU virt; the existing disk server only implements virtio-mmio"
);

pub fn init(_dma_paddr: u64) -> bool {
    false
}

pub fn ready() -> bool {
    false
}

pub fn can_flush() -> bool {
    false
}

pub fn copy_shared_to_dma(_request_slot: usize, _shared_slot: u64) {}

pub fn copy_dma_to_shared(_request_slot: usize, _shared_slot: u64) {}

pub fn request_status(_request_slot: usize) -> u8 {
    0xff
}

pub fn prepare_block_descriptor(
    _request_slot: usize,
    _blockno: u64,
    _request_type: u32,
    _data_writable_by_device: bool,
) {
}

pub fn prepare_flush_descriptor(_request_slot: usize) {}

pub fn publish_request(_request_slot: usize) {}

pub fn begin_used_drain() {}

pub fn next_used_head() -> Option<u16> {
    None
}

pub fn end_used_drain() {}

pub fn ack_irq_handler() -> bool {
    false
}

pub fn ack_virtio_interrupt() {}
