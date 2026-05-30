#![no_std]
#![no_main]
#![deny(unsafe_attr_outside_unsafe)]
#![deny(unsafe_op_in_unsafe_fn)]

use core::panic::PanicInfo;
use core::ptr;
use core::sync::atomic::{Ordering, fence};

use sel4_user::{
    IpcMessage, halt_loop, init_ipc_buffer, log, msg_info, msg_label, print_hex, print_u64,
    sel4_recv, sel4_reply_recv, sel4_yield,
};
use xv6_abi::{
    DISK_OP_GET_INFO, DISK_OP_READ, DISK_OP_WRITE, FS_BLOCK_SIZE, VIRTIO_BLK_DEVICE_ID,
    VIRTIO_BLK_F_CONFIG_WCE, VIRTIO_BLK_F_MQ, VIRTIO_BLK_F_RO, VIRTIO_BLK_F_SCSI,
    VIRTIO_BLK_SECTOR_SIZE, VIRTIO_BLK_T_IN, VIRTIO_BLK_T_OUT, VIRTIO_CONFIG_S_ACKNOWLEDGE,
    VIRTIO_CONFIG_S_DRIVER, VIRTIO_CONFIG_S_DRIVER_OK, VIRTIO_CONFIG_S_FEATURES_OK,
    VIRTIO_F_ANY_LAYOUT, VIRTIO_MMIO_DEVICE_DESC_HIGH, VIRTIO_MMIO_DEVICE_DESC_LOW,
    VIRTIO_MMIO_DEVICE_FEATURES, VIRTIO_MMIO_DEVICE_ID, VIRTIO_MMIO_DRIVER_DESC_HIGH,
    VIRTIO_MMIO_DRIVER_DESC_LOW, VIRTIO_MMIO_DRIVER_FEATURES, VIRTIO_MMIO_INTERRUPT_ACK,
    VIRTIO_MMIO_INTERRUPT_STATUS, VIRTIO_MMIO_MAGIC, VIRTIO_MMIO_MAGIC_VALUE,
    VIRTIO_MMIO_QUEUE_DESC_HIGH, VIRTIO_MMIO_QUEUE_DESC_LOW, VIRTIO_MMIO_QUEUE_NOTIFY,
    VIRTIO_MMIO_QUEUE_NUM, VIRTIO_MMIO_QUEUE_NUM_MAX, VIRTIO_MMIO_QUEUE_READY,
    VIRTIO_MMIO_QUEUE_SEL, VIRTIO_MMIO_STATUS, VIRTIO_MMIO_VENDOR_ID, VIRTIO_MMIO_VERSION,
    VIRTIO_MMIO_VERSION_MODERN, VIRTIO_QUEUE_NUM, VIRTIO_RING_F_EVENT_IDX,
    VIRTIO_RING_F_INDIRECT_DESC, VIRTQ_DESC_F_NEXT, VIRTQ_DESC_F_WRITE, XV6_ABI_VERSION,
    XV6_DISK_SHARED_BUFFER_VADDR, XV6_EINVAL, XV6_ENOSYS, XV6_FS_SIZE_BLOCKS,
    XV6_FS_TO_DISK_PROTOCOL, XV6_OK, XV6_SERVICE_ENDPOINT_CPTR, XV6_VIRTIO_DMA_VADDR,
    XV6_VIRTIO_MMIO_VADDR,
};

const DESC_OFF: u64 = 0x000;
const AVAIL_OFF: u64 = 0x100;
const USED_OFF: u64 = 0x200;
const REQ_OFF: u64 = 0x300;
const DATA_OFF: u64 = 0x400;
const STATUS_OFF: u64 = 0x900;
const VIRTIO_TIMEOUT_POLLS: usize = 10_000_000;
const TRACE_BLOCK_IO: bool = option_env!("XV6_TRACE_BLOCK_IO").is_some();

static mut DMA_PADDR: u64 = 0;
static mut USED_IDX: u16 = 0;
static mut DISK_READY: bool = false;

#[unsafe(no_mangle)]
pub extern "C" fn _start(ipc_buffer: usize, dma_paddr: usize) -> ! {
    init_ipc_buffer(ipc_buffer as u64);
    unsafe {
        DMA_PADDR = dma_paddr as u64;
        DISK_READY = init_virtio_disk();
    }
    log("virtio-disk-server: boot\n");
    log("virtio-disk-server: protocol=");
    print_u64(XV6_FS_TO_DISK_PROTOCOL);
    log(" abi=");
    print_u64(XV6_ABI_VERSION);
    log(" sector=");
    print_u64(VIRTIO_BLK_SECTOR_SIZE as u64);
    log(" first-op=");
    print_u64(DISK_OP_GET_INFO);
    log("\n");
    log("virtio-disk-server: waiting for fs-server client hookup\n");
    let mut msg = unsafe { sel4_recv(XV6_SERVICE_ENDPOINT_CPTR) };
    loop {
        let reply_mrs = handle_request(&msg);
        msg =
            unsafe { sel4_reply_recv(XV6_SERVICE_ENDPOINT_CPTR, msg_info(0, 0, 0, 4), &reply_mrs) };
    }
}

fn handle_request(msg: &IpcMessage) -> [u64; 4] {
    match msg_label(msg.info) {
        DISK_OP_GET_INFO => handle_get_info(msg),
        DISK_OP_READ => handle_read(msg),
        DISK_OP_WRITE => handle_write(msg),
        op => {
            log("virtio-disk-server: unsupported op=");
            print_u64(op);
            log("\n");
            [XV6_ENOSYS, 0, 0, 0]
        }
    }
}

fn handle_get_info(msg: &IpcMessage) -> [u64; 4] {
    if msg.mrs[0] != XV6_FS_TO_DISK_PROTOCOL || msg.mrs[1] != XV6_ABI_VERSION {
        log("virtio-disk-server: bad get-info protocol\n");
        return [XV6_EINVAL, 0, 0, 0];
    }
    if unsafe { !DISK_READY } {
        log("virtio-disk-server: get-info before ready\n");
        return [XV6_EINVAL, 0, 0, 0];
    }
    log("virtio-disk-server: get-info ready");
    log("\n");
    [
        XV6_OK,
        VIRTIO_BLK_SECTOR_SIZE as u64,
        XV6_FS_SIZE_BLOCKS as u64,
        0,
    ]
}

fn handle_read(msg: &IpcMessage) -> [u64; 4] {
    if msg.mrs[0] != XV6_FS_TO_DISK_PROTOCOL || msg.mrs[1] != XV6_ABI_VERSION {
        log("virtio-disk-server: bad read protocol\n");
        return [XV6_EINVAL, 0, 0, 0];
    }
    if unsafe { !DISK_READY } {
        log("virtio-disk-server: read before ready\n");
        return [XV6_EINVAL, 0, 0, 0];
    }
    let blockno = msg.mrs[2];
    if blockno >= XV6_FS_SIZE_BLOCKS as u64 {
        log("virtio-disk-server: read out of range block=");
        print_u64(blockno);
        log("\n");
        return [XV6_EINVAL, 0, 0, 0];
    }
    if !submit_block(blockno, VIRTIO_BLK_T_IN, true, "read") {
        return [XV6_EINVAL, 0, 0, 0];
    }
    unsafe {
        ptr::copy_nonoverlapping(
            dma_va(DATA_OFF) as *const u8,
            XV6_DISK_SHARED_BUFFER_VADDR as *mut u8,
            FS_BLOCK_SIZE,
        );
    }
    fence(Ordering::SeqCst);
    trace_block_io("read", blockno);
    [XV6_OK, FS_BLOCK_SIZE as u64, blockno, 0]
}

fn handle_write(msg: &IpcMessage) -> [u64; 4] {
    if msg.mrs[0] != XV6_FS_TO_DISK_PROTOCOL || msg.mrs[1] != XV6_ABI_VERSION {
        log("virtio-disk-server: bad write protocol\n");
        return [XV6_EINVAL, 0, 0, 0];
    }
    if unsafe { !DISK_READY } {
        log("virtio-disk-server: write before ready\n");
        return [XV6_EINVAL, 0, 0, 0];
    }
    let blockno = msg.mrs[2];
    if blockno >= XV6_FS_SIZE_BLOCKS as u64 {
        log("virtio-disk-server: write out of range block=");
        print_u64(blockno);
        log("\n");
        return [XV6_EINVAL, 0, 0, 0];
    }
    fence(Ordering::SeqCst);
    unsafe {
        ptr::copy_nonoverlapping(
            XV6_DISK_SHARED_BUFFER_VADDR as *const u8,
            dma_va(DATA_OFF) as *mut u8,
            FS_BLOCK_SIZE,
        );
    }
    fence(Ordering::SeqCst);
    if !submit_block(blockno, VIRTIO_BLK_T_OUT, false, "write") {
        return [XV6_EINVAL, 0, 0, 0];
    }
    trace_block_io("write", blockno);
    [XV6_OK, FS_BLOCK_SIZE as u64, blockno, 0]
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
        log("virtio-disk-server: FEATURES_OK rejected\n");
        return false;
    }

    mmio_write32(VIRTIO_MMIO_QUEUE_SEL, 0);
    if mmio_read32(VIRTIO_MMIO_QUEUE_READY) != 0 {
        log("virtio-disk-server: queue already ready\n");
        return false;
    }
    let queue_max = mmio_read32(VIRTIO_MMIO_QUEUE_NUM_MAX);
    if queue_max < VIRTIO_QUEUE_NUM as u32 {
        log("virtio-disk-server: queue too small max=");
        print_u64(queue_max as u64);
        log("\n");
        return false;
    }

    unsafe {
        ptr::write_bytes(XV6_VIRTIO_DMA_VADDR as *mut u8, 0, 4096);
        USED_IDX = 0;
    }

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
    log("virtio-disk-server: virtqueue ready dma=");
    print_hex(unsafe { DMA_PADDR });
    log("\n");
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
        log("virtio-disk-server: mmio vendor=");
        print_hex(vendor as u64);
        log("\n");
        return true;
    }
    log("virtio-disk-server: unexpected mmio identity magic=");
    print_hex(magic as u64);
    log(" version=");
    print_hex(version as u64);
    log(" device=");
    print_hex(device_id as u64);
    log(" vendor=");
    print_hex(vendor as u64);
    log("\n");
    false
}

fn submit_block(blockno: u64, request_type: u32, data_writable_by_device: bool, op: &str) -> bool {
    let sector = blockno * (FS_BLOCK_SIZE / VIRTIO_BLK_SECTOR_SIZE) as u64;
    write32(REQ_OFF, request_type);
    write32(REQ_OFF + 4, 0);
    write64(REQ_OFF + 8, sector);

    write_desc(0, dma_pa(REQ_OFF), 16, VIRTQ_DESC_F_NEXT, 1);
    let data_flags = if data_writable_by_device {
        VIRTQ_DESC_F_WRITE | VIRTQ_DESC_F_NEXT
    } else {
        VIRTQ_DESC_F_NEXT
    };
    write_desc(1, dma_pa(DATA_OFF), FS_BLOCK_SIZE as u32, data_flags, 2);
    unsafe {
        ptr::write_volatile(dma_va(STATUS_OFF) as *mut u8, 0xff);
    }
    write_desc(2, dma_pa(STATUS_OFF), 1, VIRTQ_DESC_F_WRITE, 0);

    let avail_idx = read16(AVAIL_OFF + 2);
    write16(
        AVAIL_OFF + 4 + ((avail_idx as u64 % VIRTIO_QUEUE_NUM as u64) * 2),
        0,
    );
    fence(Ordering::SeqCst);
    write16(AVAIL_OFF + 2, avail_idx.wrapping_add(1));
    fence(Ordering::SeqCst);
    mmio_write32(VIRTIO_MMIO_QUEUE_NOTIFY, 0);

    let mut polls = 0usize;
    while read16(USED_OFF + 2) == unsafe { USED_IDX } {
        polls += 1;
        if polls > VIRTIO_TIMEOUT_POLLS {
            log("virtio-disk-server: ");
            log(op);
            log(" timeout block=");
            print_u64(blockno);
            log("\n");
            return false;
        }
        if (polls & 0xfff) == 0 {
            unsafe { sel4_yield() };
        }
    }
    fence(Ordering::SeqCst);
    let status = unsafe { ptr::read_volatile(dma_va(STATUS_OFF) as *const u8) };
    unsafe {
        USED_IDX = USED_IDX.wrapping_add(1);
    }
    let irq_status = mmio_read32(VIRTIO_MMIO_INTERRUPT_STATUS) & 0x3;
    if irq_status != 0 {
        mmio_write32(VIRTIO_MMIO_INTERRUPT_ACK, irq_status);
    }
    status == 0
}

fn trace_block_io(op: &str, blockno: u64) {
    if !TRACE_BLOCK_IO {
        return;
    }
    log("virtio-disk-server: ");
    log(op);
    log(" block=");
    print_u64(blockno);
    log("\n");
}

fn write_queue_addr(low_reg: u64, high_reg: u64, paddr: u64) {
    mmio_write32(low_reg, paddr as u32);
    mmio_write32(high_reg, (paddr >> 32) as u32);
}

fn write_desc(index: u64, addr: u64, len: u32, flags: u16, next: u16) {
    let off = DESC_OFF + index * 16;
    write64(off, addr);
    write32(off + 8, len);
    write16(off + 12, flags);
    write16(off + 14, next);
}

fn read16(offset: u64) -> u16 {
    unsafe { ptr::read_volatile(dma_va(offset) as *const u16) }
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

fn dma_va(offset: u64) -> usize {
    (XV6_VIRTIO_DMA_VADDR + offset) as usize
}

fn dma_pa(offset: u64) -> u64 {
    unsafe { DMA_PADDR + offset }
}

#[panic_handler]
fn panic(_: &PanicInfo) -> ! {
    log("virtio-disk-server: panic\n");
    halt_loop()
}
