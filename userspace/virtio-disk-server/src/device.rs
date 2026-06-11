use core::ptr;
use core::sync::atomic::{AtomicBool, AtomicU16, AtomicU64, Ordering, fence};

use sel4_user::{LABEL_IRQ_ACK, info, msg_info, msg_label, sel4_call, warn};
use xv6_abi::{
    FS_BLOCK_SIZE, VIRTIO_BLK_DEVICE_ID, VIRTIO_BLK_F_CONFIG_WCE, VIRTIO_BLK_F_FLUSH,
    VIRTIO_BLK_F_MQ, VIRTIO_BLK_F_RO, VIRTIO_BLK_F_SCSI, VIRTIO_BLK_SECTOR_SIZE,
    VIRTIO_BLK_T_FLUSH, VIRTIO_CONFIG_S_ACKNOWLEDGE, VIRTIO_CONFIG_S_DRIVER,
    VIRTIO_CONFIG_S_DRIVER_OK, VIRTIO_CONFIG_S_FEATURES_OK, VIRTIO_F_ANY_LAYOUT,
    VIRTIO_MMIO_DEVICE_DESC_HIGH, VIRTIO_MMIO_DEVICE_DESC_LOW, VIRTIO_MMIO_DEVICE_FEATURES,
    VIRTIO_MMIO_DEVICE_ID, VIRTIO_MMIO_DRIVER_DESC_HIGH, VIRTIO_MMIO_DRIVER_DESC_LOW,
    VIRTIO_MMIO_DRIVER_FEATURES, VIRTIO_MMIO_INTERRUPT_ACK, VIRTIO_MMIO_INTERRUPT_STATUS,
    VIRTIO_MMIO_MAGIC, VIRTIO_MMIO_MAGIC_VALUE, VIRTIO_MMIO_QUEUE_DESC_HIGH,
    VIRTIO_MMIO_QUEUE_DESC_LOW, VIRTIO_MMIO_QUEUE_NOTIFY, VIRTIO_MMIO_QUEUE_NUM,
    VIRTIO_MMIO_QUEUE_NUM_MAX, VIRTIO_MMIO_QUEUE_READY, VIRTIO_MMIO_QUEUE_SEL, VIRTIO_MMIO_STATUS,
    VIRTIO_MMIO_VENDOR_ID, VIRTIO_MMIO_VERSION, VIRTIO_MMIO_VERSION_MODERN, VIRTIO_QUEUE_NUM,
    VIRTIO_RING_F_EVENT_IDX, VIRTIO_RING_F_INDIRECT_DESC, VIRTQ_DESC_F_NEXT, VIRTQ_DESC_F_WRITE,
    XV6_DISK_IRQ_HANDLER_CPTR, XV6_DISK_MAX_IN_FLIGHT, XV6_VIRTIO_DMA_VADDR, XV6_VIRTIO_MMIO_VADDR,
};

use crate::layout::{
    AVAIL_OFF, DESC_OFF, DESCS_PER_REQUEST, USED_OFF, data_off, desc_head, dma_va, req_off,
    shared_buffer_va, status_off,
};

static DMA_PADDR: AtomicU64 = AtomicU64::new(0);
static USED_IDX: AtomicU16 = AtomicU16::new(0);
static DISK_READY: AtomicBool = AtomicBool::new(false);
static DISK_CAN_FLUSH: AtomicBool = AtomicBool::new(false);

pub fn init(dma_paddr: u64) -> bool {
    DMA_PADDR.store(dma_paddr, Ordering::Relaxed);
    let ready = init_virtio_disk();
    DISK_READY.store(ready, Ordering::Relaxed);
    ready
}

pub fn ready() -> bool {
    DISK_READY.load(Ordering::Relaxed)
}

pub fn can_flush() -> bool {
    DISK_CAN_FLUSH.load(Ordering::Relaxed)
}

pub fn copy_shared_to_dma(request_slot: usize, shared_slot: u64) {
    fence(Ordering::SeqCst);
    unsafe {
        ptr::copy_nonoverlapping(
            shared_buffer_va(shared_slot) as *const u8,
            dma_va(data_off(request_slot)) as *mut u8,
            FS_BLOCK_SIZE,
        );
    }
    fence(Ordering::SeqCst);
}

pub fn copy_dma_to_shared(request_slot: usize, shared_slot: u64) {
    unsafe {
        ptr::copy_nonoverlapping(
            dma_va(data_off(request_slot)) as *const u8,
            shared_buffer_va(shared_slot) as *mut u8,
            FS_BLOCK_SIZE,
        );
    }
    fence(Ordering::SeqCst);
}

pub fn request_status(request_slot: usize) -> u8 {
    unsafe { ptr::read_volatile(dma_va(status_off(request_slot)) as *const u8) }
}

pub fn prepare_block_descriptor(
    request_slot: usize,
    blockno: u64,
    request_type: u32,
    data_writable_by_device: bool,
) {
    let sector = blockno * (FS_BLOCK_SIZE / VIRTIO_BLK_SECTOR_SIZE) as u64;
    let head = desc_head(request_slot);
    write32(req_off(request_slot), request_type);
    write32(req_off(request_slot) + 4, 0);
    write64(req_off(request_slot) + 8, sector);

    write_desc(
        head,
        dma_pa(req_off(request_slot)),
        16,
        VIRTQ_DESC_F_NEXT,
        head + 1,
    );
    let data_flags = if data_writable_by_device {
        VIRTQ_DESC_F_WRITE | VIRTQ_DESC_F_NEXT
    } else {
        VIRTQ_DESC_F_NEXT
    };
    write_desc(
        head + 1,
        dma_pa(data_off(request_slot)),
        FS_BLOCK_SIZE as u32,
        data_flags,
        head + 2,
    );
    unsafe {
        ptr::write_volatile(dma_va(status_off(request_slot)) as *mut u8, 0xff);
    }
    write_desc(
        head + 2,
        dma_pa(status_off(request_slot)),
        1,
        VIRTQ_DESC_F_WRITE,
        0,
    );
}

pub fn prepare_flush_descriptor(request_slot: usize) {
    let head = desc_head(request_slot);
    write32(req_off(request_slot), VIRTIO_BLK_T_FLUSH);
    write32(req_off(request_slot) + 4, 0);
    write64(req_off(request_slot) + 8, 0);

    write_desc(
        head,
        dma_pa(req_off(request_slot)),
        16,
        VIRTQ_DESC_F_NEXT,
        head + 1,
    );
    unsafe {
        ptr::write_volatile(dma_va(status_off(request_slot)) as *mut u8, 0xff);
    }
    write_desc(
        head + 1,
        dma_pa(status_off(request_slot)),
        1,
        VIRTQ_DESC_F_WRITE,
        0,
    );
}

pub fn publish_request(request_slot: usize) {
    let head = desc_head(request_slot);
    let avail_idx = read16(AVAIL_OFF + 2);
    write16(
        AVAIL_OFF + 4 + ((avail_idx as u64 % VIRTIO_QUEUE_NUM as u64) * 2),
        head,
    );
    fence(Ordering::SeqCst);
    write16(AVAIL_OFF + 2, avail_idx.wrapping_add(1));
    fence(Ordering::SeqCst);

    mmio_write32(VIRTIO_MMIO_QUEUE_NOTIFY, 0);
}

pub fn begin_used_drain() {
    fence(Ordering::SeqCst);
}

pub fn next_used_head() -> Option<u16> {
    let used_idx = read16(USED_OFF + 2);
    let next_used = USED_IDX.load(Ordering::Relaxed);
    if next_used == used_idx {
        return None;
    }
    let ring_index = next_used as u64 % VIRTIO_QUEUE_NUM as u64;
    let head = read32(USED_OFF + 4 + ring_index * 8) as u16;
    USED_IDX.store(next_used.wrapping_add(1), Ordering::Relaxed);
    Some(head)
}

pub fn end_used_drain() {
    fence(Ordering::SeqCst);
}

pub fn ack_irq_handler() -> bool {
    let reply = unsafe {
        sel4_call(
            XV6_DISK_IRQ_HANDLER_CPTR,
            msg_info(LABEL_IRQ_ACK, 0, 0, 0),
            &[],
        )
    };
    if msg_label(reply.info) != 0 {
        warn!(
            "virtio-disk-server: irq ack failed label={}",
            msg_label(reply.info)
        );
        return false;
    }
    true
}

pub fn ack_virtio_interrupt() {
    let irq_status = mmio_read32(VIRTIO_MMIO_INTERRUPT_STATUS) & 0x3;
    if irq_status != 0 {
        mmio_write32(VIRTIO_MMIO_INTERRUPT_ACK, irq_status);
    }
}

fn init_virtio_disk() -> bool {
    if !check_identity() {
        return false;
    }
    let mut status = 0u32;
    mmio_write32(VIRTIO_MMIO_STATUS, status);

    status |= VIRTIO_CONFIG_S_ACKNOWLEDGE;
    mmio_write32(VIRTIO_MMIO_STATUS, status);
    status |= VIRTIO_CONFIG_S_DRIVER;
    mmio_write32(VIRTIO_MMIO_STATUS, status);

    let mut features = mmio_read32(VIRTIO_MMIO_DEVICE_FEATURES);
    features &= !(1 << VIRTIO_BLK_F_RO);
    features &= !(1 << VIRTIO_BLK_F_SCSI);
    features &= !(1 << VIRTIO_BLK_F_CONFIG_WCE);
    features &= !(1 << VIRTIO_BLK_F_MQ);
    features &= !(1 << VIRTIO_F_ANY_LAYOUT);
    features &= !(1 << VIRTIO_RING_F_INDIRECT_DESC);
    features &= !(1 << VIRTIO_RING_F_EVENT_IDX);
    mmio_write32(VIRTIO_MMIO_DRIVER_FEATURES, features);

    status |= VIRTIO_CONFIG_S_FEATURES_OK;
    mmio_write32(VIRTIO_MMIO_STATUS, status);
    if (mmio_read32(VIRTIO_MMIO_STATUS) & VIRTIO_CONFIG_S_FEATURES_OK) == 0 {
        warn!("virtio-disk-server: FEATURES_OK rejected");
        return false;
    }
    DISK_CAN_FLUSH.store(
        (features & (1 << VIRTIO_BLK_F_FLUSH)) != 0,
        Ordering::Relaxed,
    );

    mmio_write32(VIRTIO_MMIO_QUEUE_SEL, 0);
    if mmio_read32(VIRTIO_MMIO_QUEUE_READY) != 0 {
        warn!("virtio-disk-server: queue already ready");
        return false;
    }
    let queue_max = mmio_read32(VIRTIO_MMIO_QUEUE_NUM_MAX);
    if queue_max < VIRTIO_QUEUE_NUM as u32 {
        warn!("virtio-disk-server: queue too small max={}", queue_max);
        return false;
    }
    if VIRTIO_QUEUE_NUM < XV6_DISK_MAX_IN_FLIGHT * DESCS_PER_REQUEST as usize {
        warn!("virtio-disk-server: queue too small for request slots");
        return false;
    }

    unsafe {
        ptr::write_bytes(XV6_VIRTIO_DMA_VADDR as *mut u8, 0, 4096);
    }
    USED_IDX.store(0, Ordering::Relaxed);

    mmio_write32(VIRTIO_MMIO_QUEUE_NUM, VIRTIO_QUEUE_NUM as u32);
    write_queue_addr(
        VIRTIO_MMIO_QUEUE_DESC_LOW,
        VIRTIO_MMIO_QUEUE_DESC_HIGH,
        dma_pa(DESC_OFF),
    );
    write_queue_addr(
        VIRTIO_MMIO_DRIVER_DESC_LOW,
        VIRTIO_MMIO_DRIVER_DESC_HIGH,
        dma_pa(AVAIL_OFF),
    );
    write_queue_addr(
        VIRTIO_MMIO_DEVICE_DESC_LOW,
        VIRTIO_MMIO_DEVICE_DESC_HIGH,
        dma_pa(USED_OFF),
    );
    mmio_write32(VIRTIO_MMIO_QUEUE_READY, 1);

    status |= VIRTIO_CONFIG_S_DRIVER_OK;
    mmio_write32(VIRTIO_MMIO_STATUS, status);
    info!(
        "virtio-disk-server: virtqueue ready dma={:#x}",
        DMA_PADDR.load(Ordering::Relaxed)
    );
    true
}

fn check_identity() -> bool {
    let magic = mmio_read32(VIRTIO_MMIO_MAGIC_VALUE);
    let version = mmio_read32(VIRTIO_MMIO_VERSION);
    let device_id = mmio_read32(VIRTIO_MMIO_DEVICE_ID);
    let vendor = mmio_read32(VIRTIO_MMIO_VENDOR_ID);
    if magic == VIRTIO_MMIO_MAGIC
        && version == VIRTIO_MMIO_VERSION_MODERN
        && device_id == VIRTIO_BLK_DEVICE_ID
    {
        info!("virtio-disk-server: mmio vendor={:#x}", vendor);
        return true;
    }
    warn!(
        "virtio-disk-server: unexpected mmio identity magic={:#x} version={:#x} device={:#x} vendor={:#x}",
        magic, version, device_id, vendor
    );
    false
}

fn write_queue_addr(low_reg: u64, high_reg: u64, paddr: u64) {
    mmio_write32(low_reg, paddr as u32);
    mmio_write32(high_reg, (paddr >> 32) as u32);
}

fn write_desc(index: u16, addr: u64, len: u32, flags: u16, next: u16) {
    let off = DESC_OFF + index as u64 * 16;
    write64(off, addr);
    write32(off + 8, len);
    write16(off + 12, flags);
    write16(off + 14, next);
}

fn read16(offset: u64) -> u16 {
    unsafe { ptr::read_volatile(dma_va(offset) as *const u16) }
}

fn read32(offset: u64) -> u32 {
    unsafe { ptr::read_volatile(dma_va(offset) as *const u32) }
}

fn write16(offset: u64, value: u16) {
    unsafe { ptr::write_volatile(dma_va(offset) as *mut u16, value) }
}

fn write32(offset: u64, value: u32) {
    unsafe { ptr::write_volatile(dma_va(offset) as *mut u32, value) }
}

fn write64(offset: u64, value: u64) {
    unsafe { ptr::write_volatile(dma_va(offset) as *mut u64, value) }
}

fn mmio_read32(offset: u64) -> u32 {
    unsafe { ptr::read_volatile((XV6_VIRTIO_MMIO_VADDR + offset) as *const u32) }
}

fn mmio_write32(offset: u64, value: u32) {
    unsafe { ptr::write_volatile((XV6_VIRTIO_MMIO_VADDR + offset) as *mut u32, value) }
}

fn dma_pa(offset: u64) -> u64 {
    DMA_PADDR.load(Ordering::Relaxed) + offset
}
