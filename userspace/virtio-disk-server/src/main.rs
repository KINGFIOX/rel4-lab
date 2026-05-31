#![no_std]
#![no_main]
#![deny(unsafe_attr_outside_unsafe)]
#![deny(unsafe_op_in_unsafe_fn)]

use core::panic::PanicInfo;
use core::ptr;
use core::sync::atomic::{Ordering, fence};

use sel4_user::{
    IpcMessage, LABEL_CNODE_SAVE_CALLER, LABEL_IRQ_ACK, ROOT_CNODE_DEPTH, call_checked, halt_loop,
    init_ipc_buffer, log, msg_info, msg_label, msg_len, print_hex, print_u64, sel4_call, sel4_recv,
    sel4_reply_recv, sel4_send,
};
use xv6_abi::{
    DISK_OP_COMPLETE, DISK_OP_FLUSH, DISK_OP_GET_INFO, DISK_OP_READ, DISK_OP_WRITE, FS_BLOCK_SIZE,
    VIRTIO_BLK_DEVICE_ID, VIRTIO_BLK_F_CONFIG_WCE, VIRTIO_BLK_F_FLUSH, VIRTIO_BLK_F_MQ,
    VIRTIO_BLK_F_RO, VIRTIO_BLK_F_SCSI, VIRTIO_BLK_SECTOR_SIZE, VIRTIO_BLK_T_FLUSH,
    VIRTIO_BLK_T_IN, VIRTIO_BLK_T_OUT, VIRTIO_CONFIG_S_ACKNOWLEDGE, VIRTIO_CONFIG_S_DRIVER,
    VIRTIO_CONFIG_S_DRIVER_OK, VIRTIO_CONFIG_S_FEATURES_OK, VIRTIO_F_ANY_LAYOUT,
    VIRTIO_MMIO_DEVICE_DESC_HIGH, VIRTIO_MMIO_DEVICE_DESC_LOW, VIRTIO_MMIO_DEVICE_FEATURES,
    VIRTIO_MMIO_DEVICE_ID, VIRTIO_MMIO_DRIVER_DESC_HIGH, VIRTIO_MMIO_DRIVER_DESC_LOW,
    VIRTIO_MMIO_DRIVER_FEATURES, VIRTIO_MMIO_INTERRUPT_ACK, VIRTIO_MMIO_INTERRUPT_STATUS,
    VIRTIO_MMIO_MAGIC, VIRTIO_MMIO_MAGIC_VALUE, VIRTIO_MMIO_QUEUE_DESC_HIGH,
    VIRTIO_MMIO_QUEUE_DESC_LOW, VIRTIO_MMIO_QUEUE_NOTIFY, VIRTIO_MMIO_QUEUE_NUM,
    VIRTIO_MMIO_QUEUE_NUM_MAX, VIRTIO_MMIO_QUEUE_READY, VIRTIO_MMIO_QUEUE_SEL, VIRTIO_MMIO_STATUS,
    VIRTIO_MMIO_VENDOR_ID, VIRTIO_MMIO_VERSION, VIRTIO_MMIO_VERSION_MODERN, VIRTIO_QUEUE_NUM,
    VIRTIO_RING_F_EVENT_IDX, VIRTIO_RING_F_INDIRECT_DESC, VIRTQ_DESC_F_NEXT, VIRTQ_DESC_F_WRITE,
    XV6_ABI_VERSION, XV6_DISK_COMPLETION_ENTRY_WORDS, XV6_DISK_COMPLETION_NTFN_CPTR,
    XV6_DISK_COMPLETION_RING_ENTRIES, XV6_DISK_COMPLETION_RING_VADDR, XV6_DISK_IRQ_BADGE,
    XV6_DISK_IRQ_HANDLER_CPTR, XV6_DISK_MAX_IN_FLIGHT, XV6_DISK_SHARED_BUFFER_SLOTS,
    XV6_DISK_SHARED_BUFFER_VADDR, XV6_EINVAL, XV6_ENOSYS, XV6_FS_SIZE_BLOCKS,
    XV6_FS_TO_DISK_PROTOCOL, XV6_OK, XV6_SERVER_CNODE_CPTR, XV6_SERVER_REPLY_CPTR,
    XV6_SERVICE_ENDPOINT_CPTR, XV6_VIRTIO_DMA_VADDR, XV6_VIRTIO_MMIO_VADDR,
};

const DESC_OFF: u64 = 0x000;
const AVAIL_OFF: u64 = 0x100;
const USED_OFF: u64 = 0x200;
const REQUEST_AREA_OFF: u64 = 0x300;
const REQUEST_STRIDE: u64 = 0x600;
const REQ_REL_OFF: u64 = 0x000;
const DATA_REL_OFF: u64 = 0x100;
const STATUS_REL_OFF: u64 = 0x500;
const DESCS_PER_REQUEST: u16 = 3;
const COMPLETION_WRITE_IDX_OFF: u64 = 0;
const COMPLETION_READ_IDX_OFF: u64 = 8;
const COMPLETION_ENTRIES_OFF: u64 = 16;
const COMPLETION_ENTRY_STRIDE: u64 = (XV6_DISK_COMPLETION_ENTRY_WORDS as u64) * 8;
const TRACE_BLOCK_IO: bool = option_env!("XV6_TRACE_BLOCK_IO").is_some();

static mut DMA_PADDR: u64 = 0;
static mut USED_IDX: u16 = 0;
static mut DISK_READY: bool = false;
static mut DISK_CAN_FLUSH: bool = false;

static mut PENDING_REQUESTS: [PendingRequest; XV6_DISK_MAX_IN_FLIGHT] =
    [PendingRequest::none(); XV6_DISK_MAX_IN_FLIGHT];

#[derive(Copy, Clone, PartialEq, Eq)]
enum DiskOp {
    Read,
    Write,
    Flush,
}

impl DiskOp {
    const fn name(self) -> &'static str {
        match self {
            Self::Read => "read",
            Self::Write => "write",
            Self::Flush => "flush",
        }
    }
}

#[derive(Copy, Clone)]
struct PendingRequest {
    active: bool,
    op: DiskOp,
    blockno: u64,
    shared_slot: u64,
    completion_id: u64,
    async_completion: bool,
}

impl PendingRequest {
    const fn none() -> Self {
        Self {
            active: false,
            op: DiskOp::Read,
            blockno: 0,
            shared_slot: 0,
            completion_id: 0,
            async_completion: false,
        }
    }
}

#[derive(Copy, Clone)]
struct ReplyTarget {
    async_completion: bool,
    completion_id: u64,
}

impl ReplyTarget {
    const fn caller() -> Self {
        Self {
            async_completion: false,
            completion_id: 0,
        }
    }

    const fn completion(completion_id: u64) -> Self {
        Self {
            async_completion: true,
            completion_id,
        }
    }
}

enum RequestResult {
    Reply([u64; 4]),
    Deferred,
}

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
    let mut reply_pending = false;
    let mut reply_mrs = [0u64; 4];
    loop {
        let msg = if reply_pending {
            reply_pending = false;
            unsafe { sel4_reply_recv(XV6_SERVICE_ENDPOINT_CPTR, msg_info(0, 0, 0, 4), &reply_mrs) }
        } else {
            unsafe { sel4_recv(XV6_SERVICE_ENDPOINT_CPTR) }
        };

        if is_disk_irq(&msg) {
            handle_disk_irq();
            continue;
        }
        if msg_label(msg.info) == 0 {
            log("virtio-disk-server: unexpected zero-label IPC badge=");
            print_hex(msg.badge);
            log("\n");
            continue;
        }

        match handle_request(&msg) {
            RequestResult::Reply(mrs) => {
                reply_mrs = mrs;
                reply_pending = true;
            }
            RequestResult::Deferred => {}
        }
    }
}

fn handle_request(msg: &IpcMessage) -> RequestResult {
    harvest_visible_completions_before_rpc();

    match msg_label(msg.info) {
        DISK_OP_GET_INFO => RequestResult::Reply(handle_get_info(msg)),
        DISK_OP_READ => handle_read(msg),
        DISK_OP_WRITE => handle_write(msg),
        DISK_OP_FLUSH => handle_flush(msg),
        op => {
            log("virtio-disk-server: unsupported op=");
            print_u64(op);
            log("\n");
            RequestResult::Reply([XV6_ENOSYS, 0, 0, 0])
        }
    }
}

fn harvest_visible_completions_before_rpc() {
    // This is not a DMA wait path. The server returns to seL4_Recv while the
    // device owns requests, and completions normally arrive via the bound IRQ
    // notification. This non-blocking harvest only handles an ordering case:
    // a used-ring entry can become visible before its IRQ badge is the next
    // message delivered on the service endpoint, while another client RPC is
    // already queued. Draining visible completions first keeps request-slot and
    // shared-slot ownership current without spinning.
    if !any_request_in_flight() {
        return;
    }
    let _ = complete_used_requests();
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

fn handle_read(msg: &IpcMessage) -> RequestResult {
    let target = data_reply_target(msg);
    if msg.mrs[0] != XV6_FS_TO_DISK_PROTOCOL || msg.mrs[1] != XV6_ABI_VERSION {
        log("virtio-disk-server: bad read protocol\n");
        return immediate_reply(target, [XV6_EINVAL, 0, 0, 0]);
    }
    if unsafe { !DISK_READY } {
        log("virtio-disk-server: read before ready\n");
        return immediate_reply(target, [XV6_EINVAL, 0, 0, 0]);
    }
    let blockno = msg.mrs[2];
    if blockno >= XV6_FS_SIZE_BLOCKS as u64 {
        log("virtio-disk-server: read out of range block=");
        print_u64(blockno);
        log("\n");
        return immediate_reply(target, [XV6_EINVAL, 0, 0, 0]);
    }
    let Some(shared_slot) = parse_shared_slot(msg) else {
        return immediate_reply(target, [XV6_EINVAL, 0, blockno, 0]);
    };
    if shared_slot_in_use(shared_slot) {
        reject_busy_shared_slot("read", shared_slot, blockno);
        return immediate_reply(target, [XV6_EINVAL, 0, blockno, 0]);
    }
    let Some(request_slot) = alloc_request_slot() else {
        reject_no_request_slot("read", blockno);
        return immediate_reply(target, [XV6_EINVAL, 0, blockno, 0]);
    };
    save_pending_reply_target(request_slot, target);
    if !submit_block_async(
        request_slot,
        blockno,
        shared_slot,
        VIRTIO_BLK_T_IN,
        true,
        DiskOp::Read,
        target,
    ) {
        send_deferred_reply_target(request_slot, target, [XV6_EINVAL, 0, blockno, 0]);
    }
    RequestResult::Deferred
}

fn handle_write(msg: &IpcMessage) -> RequestResult {
    let target = data_reply_target(msg);
    if msg.mrs[0] != XV6_FS_TO_DISK_PROTOCOL || msg.mrs[1] != XV6_ABI_VERSION {
        log("virtio-disk-server: bad write protocol\n");
        return immediate_reply(target, [XV6_EINVAL, 0, 0, 0]);
    }
    if unsafe { !DISK_READY } {
        log("virtio-disk-server: write before ready\n");
        return immediate_reply(target, [XV6_EINVAL, 0, 0, 0]);
    }
    let blockno = msg.mrs[2];
    if blockno >= XV6_FS_SIZE_BLOCKS as u64 {
        log("virtio-disk-server: write out of range block=");
        print_u64(blockno);
        log("\n");
        return immediate_reply(target, [XV6_EINVAL, 0, 0, 0]);
    }
    let Some(shared_slot) = parse_shared_slot(msg) else {
        return immediate_reply(target, [XV6_EINVAL, 0, blockno, 0]);
    };
    if shared_slot_in_use(shared_slot) {
        reject_busy_shared_slot("write", shared_slot, blockno);
        return immediate_reply(target, [XV6_EINVAL, 0, blockno, 0]);
    }
    let Some(request_slot) = alloc_request_slot() else {
        reject_no_request_slot("write", blockno);
        return immediate_reply(target, [XV6_EINVAL, 0, blockno, 0]);
    };
    fence(Ordering::SeqCst);
    unsafe {
        ptr::copy_nonoverlapping(
            shared_buffer_va(shared_slot) as *const u8,
            dma_va(data_off(request_slot)) as *mut u8,
            FS_BLOCK_SIZE,
        );
    }
    fence(Ordering::SeqCst);
    save_pending_reply_target(request_slot, target);
    if !submit_block_async(
        request_slot,
        blockno,
        shared_slot,
        VIRTIO_BLK_T_OUT,
        false,
        DiskOp::Write,
        target,
    ) {
        send_deferred_reply_target(request_slot, target, [XV6_EINVAL, 0, blockno, 0]);
    }
    RequestResult::Deferred
}

fn handle_flush(msg: &IpcMessage) -> RequestResult {
    let target = flush_reply_target(msg);
    if msg.mrs[0] != XV6_FS_TO_DISK_PROTOCOL || msg.mrs[1] != XV6_ABI_VERSION {
        log("virtio-disk-server: bad flush protocol\n");
        return immediate_reply(target, [XV6_EINVAL, 0, 0, 0]);
    }
    if unsafe { !DISK_READY } {
        log("virtio-disk-server: flush before ready\n");
        return immediate_reply(target, [XV6_EINVAL, 0, 0, 0]);
    }
    if any_request_in_flight() {
        log("virtio-disk-server: flush rejected while request in flight\n");
        return immediate_reply(target, [XV6_EINVAL, 0, 0, 0]);
    }
    if unsafe { !DISK_CAN_FLUSH } {
        return immediate_reply(target, [XV6_OK, 0, 0, 0]);
    }

    let Some(request_slot) = alloc_request_slot() else {
        reject_no_request_slot("flush", 0);
        return immediate_reply(target, [XV6_EINVAL, 0, 0, 0]);
    };
    save_pending_reply_target(request_slot, target);
    if !submit_flush_async(request_slot, target) {
        send_deferred_reply_target(request_slot, target, [XV6_EINVAL, 0, 0, 0]);
    }
    RequestResult::Deferred
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
    unsafe {
        DISK_CAN_FLUSH = (features & (1 << VIRTIO_BLK_F_FLUSH)) != 0;
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
    if VIRTIO_QUEUE_NUM < XV6_DISK_MAX_IN_FLIGHT * DESCS_PER_REQUEST as usize {
        log("virtio-disk-server: queue too small for request slots\n");
        return false;
    }

    unsafe {
        ptr::write_bytes(XV6_VIRTIO_DMA_VADDR as *mut u8, 0, 4096);
        USED_IDX = 0;
        PENDING_REQUESTS = [PendingRequest::none(); XV6_DISK_MAX_IN_FLIGHT];
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

fn any_request_in_flight() -> bool {
    let mut i = 0usize;
    while i < XV6_DISK_MAX_IN_FLIGHT {
        if unsafe { PENDING_REQUESTS[i].active } {
            return true;
        }
        i += 1;
    }
    false
}

fn alloc_request_slot() -> Option<usize> {
    let mut i = 0usize;
    while i < XV6_DISK_MAX_IN_FLIGHT {
        if unsafe { !PENDING_REQUESTS[i].active } {
            return Some(i);
        }
        i += 1;
    }
    None
}

fn shared_slot_in_use(shared_slot: u64) -> bool {
    let mut i = 0usize;
    while i < XV6_DISK_MAX_IN_FLIGHT {
        let pending = unsafe { PENDING_REQUESTS[i] };
        if pending.active && pending.op != DiskOp::Flush && pending.shared_slot == shared_slot {
            return true;
        }
        i += 1;
    }
    false
}

fn parse_shared_slot(msg: &IpcMessage) -> Option<u64> {
    let shared_slot = if msg_len(msg.info) >= 4 {
        msg.mrs[3]
    } else {
        0
    };
    if shared_slot < XV6_DISK_SHARED_BUFFER_SLOTS as u64 {
        Some(shared_slot)
    } else {
        log("virtio-disk-server: bad shared slot=");
        print_u64(shared_slot);
        log("\n");
        None
    }
}

fn data_reply_target(msg: &IpcMessage) -> ReplyTarget {
    if msg_len(msg.info) >= 5 && msg.mrs[4] != 0 {
        ReplyTarget::completion(msg.mrs[4])
    } else {
        ReplyTarget::caller()
    }
}

fn flush_reply_target(msg: &IpcMessage) -> ReplyTarget {
    if msg_len(msg.info) >= 3 && msg.mrs[2] != 0 {
        ReplyTarget::completion(msg.mrs[2])
    } else {
        ReplyTarget::caller()
    }
}

fn immediate_reply(target: ReplyTarget, reply_mrs: [u64; 4]) -> RequestResult {
    if target.async_completion {
        send_completion(target.completion_id, reply_mrs);
        RequestResult::Deferred
    } else {
        RequestResult::Reply(reply_mrs)
    }
}

fn reject_busy_shared_slot(op: &str, shared_slot: u64, blockno: u64) {
    log("virtio-disk-server: ");
    log(op);
    log(" shared slot busy slot=");
    print_u64(shared_slot);
    log(" block=");
    print_u64(blockno);
    log("\n");
}

fn reject_no_request_slot(op: &str, blockno: u64) {
    log("virtio-disk-server: ");
    log(op);
    log(" request slots exhausted block=");
    print_u64(blockno);
    log("\n");
}

fn save_pending_reply(request_slot: usize) {
    call_checked(
        XV6_SERVER_CNODE_CPTR,
        LABEL_CNODE_SAVE_CALLER,
        &[],
        &[reply_cptr(request_slot), ROOT_CNODE_DEPTH],
    );
}

fn save_pending_reply_target(request_slot: usize, target: ReplyTarget) {
    if !target.async_completion {
        save_pending_reply(request_slot);
    }
}

fn send_deferred_reply(request_slot: usize, reply_mrs: [u64; 4]) {
    unsafe {
        sel4_send(
            reply_cptr(request_slot),
            msg_info(0, 0, 0, reply_mrs.len() as u64),
            &reply_mrs,
        );
    }
}

fn send_deferred_reply_target(request_slot: usize, target: ReplyTarget, reply_mrs: [u64; 4]) {
    if target.async_completion {
        send_completion(target.completion_id, reply_mrs);
    } else {
        send_deferred_reply(request_slot, reply_mrs);
    }
}

fn send_completion(completion_id: u64, reply_mrs: [u64; 4]) {
    if !enqueue_completion(completion_id, reply_mrs) {
        return;
    }
    trace_completion_signal(completion_id);
    unsafe {
        sel4_send(
            XV6_DISK_COMPLETION_NTFN_CPTR,
            msg_info(DISK_OP_COMPLETE, 0, 0, 0),
            &[],
        );
    }
    trace_completion_signaled(completion_id);
}

fn enqueue_completion(completion_id: u64, reply_mrs: [u64; 4]) -> bool {
    let read_idx = completion_read64(COMPLETION_READ_IDX_OFF);
    let write_idx = completion_read64(COMPLETION_WRITE_IDX_OFF);
    if write_idx.wrapping_sub(read_idx) >= XV6_DISK_COMPLETION_RING_ENTRIES as u64 {
        log("virtio-disk-server: completion ring full id=");
        print_u64(completion_id);
        log("\n");
        return false;
    }

    let slot = write_idx % XV6_DISK_COMPLETION_RING_ENTRIES as u64;
    let entry = COMPLETION_ENTRIES_OFF + slot * COMPLETION_ENTRY_STRIDE;
    completion_write64(entry, reply_mrs[0]);
    completion_write64(entry + 8, reply_mrs[1]);
    completion_write64(entry + 16, reply_mrs[2]);
    completion_write64(entry + 24, completion_id);
    completion_write64(entry + 32, reply_mrs[3]);
    fence(Ordering::SeqCst);
    completion_write64(COMPLETION_WRITE_IDX_OFF, write_idx.wrapping_add(1));
    fence(Ordering::SeqCst);
    true
}

fn submit_block_async(
    request_slot: usize,
    blockno: u64,
    shared_slot: u64,
    request_type: u32,
    data_writable_by_device: bool,
    op: DiskOp,
    target: ReplyTarget,
) -> bool {
    if unsafe { PENDING_REQUESTS[request_slot].active } {
        reject_no_request_slot(op.name(), blockno);
        return false;
    }

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

    // Mark the request active before publishing its descriptor head. The server
    // is single-threaded, but the device can DMA/interrupt as soon as avail.idx
    // is visible.
    unsafe {
        PENDING_REQUESTS[request_slot] = PendingRequest {
            active: true,
            op,
            blockno,
            shared_slot,
            completion_id: target.completion_id,
            async_completion: target.async_completion,
        };
    }

    let avail_idx = read16(AVAIL_OFF + 2);
    write16(
        AVAIL_OFF + 4 + ((avail_idx as u64 % VIRTIO_QUEUE_NUM as u64) * 2),
        head,
    );
    fence(Ordering::SeqCst);
    write16(AVAIL_OFF + 2, avail_idx.wrapping_add(1));
    fence(Ordering::SeqCst);

    mmio_write32(VIRTIO_MMIO_QUEUE_NOTIFY, 0);
    true
}

fn submit_flush_async(request_slot: usize, target: ReplyTarget) -> bool {
    if unsafe { PENDING_REQUESTS[request_slot].active } {
        reject_no_request_slot("flush", 0);
        return false;
    }

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

    // Same ordering rule as read/write: pending state must exist before the
    // descriptor head can appear in the avail ring.
    unsafe {
        PENDING_REQUESTS[request_slot] = PendingRequest {
            active: true,
            op: DiskOp::Flush,
            blockno: 0,
            shared_slot: 0,
            completion_id: target.completion_id,
            async_completion: target.async_completion,
        };
    }

    let avail_idx = read16(AVAIL_OFF + 2);
    write16(
        AVAIL_OFF + 4 + ((avail_idx as u64 % VIRTIO_QUEUE_NUM as u64) * 2),
        head,
    );
    fence(Ordering::SeqCst);
    write16(AVAIL_OFF + 2, avail_idx.wrapping_add(1));
    fence(Ordering::SeqCst);

    mmio_write32(VIRTIO_MMIO_QUEUE_NOTIFY, 0);
    true
}

fn is_disk_irq(msg: &IpcMessage) -> bool {
    msg_label(msg.info) == 0 && (msg.badge & XV6_DISK_IRQ_BADGE) != 0
}

fn handle_disk_irq() {
    ack_virtio_interrupt();
    if !ack_irq_handler() {
        return;
    }
    let _ = complete_used_requests();
}

fn complete_used_requests() -> bool {
    fence(Ordering::SeqCst);
    let used_idx = read16(USED_OFF + 2);
    let mut completed = false;
    while unsafe { USED_IDX } != used_idx {
        let ring_index = unsafe { USED_IDX } as u64 % VIRTIO_QUEUE_NUM as u64;
        let head = read32(USED_OFF + 4 + ring_index * 8) as u16;
        unsafe {
            USED_IDX = USED_IDX.wrapping_add(1);
        }
        if let Some((request_slot, target, reply_mrs)) = complete_request_by_head(head) {
            send_deferred_reply_target(request_slot, target, reply_mrs);
            completed = true;
        } else {
            log("virtio-disk-server: used entry for unknown head=");
            print_u64(head as u64);
            log("\n");
        }
    }
    fence(Ordering::SeqCst);
    completed
}

fn complete_request_by_head(head: u16) -> Option<(usize, ReplyTarget, [u64; 4])> {
    let request_slot = request_slot_from_head(head)?;
    let pending = unsafe { PENDING_REQUESTS[request_slot] };
    if !pending.active {
        return None;
    }
    let target = ReplyTarget {
        async_completion: pending.async_completion,
        completion_id: pending.completion_id,
    };

    let status = unsafe { ptr::read_volatile(dma_va(status_off(request_slot)) as *const u8) };
    if status == 0 && pending.op == DiskOp::Read {
        unsafe {
            ptr::copy_nonoverlapping(
                dma_va(data_off(request_slot)) as *const u8,
                shared_buffer_va(pending.shared_slot) as *mut u8,
                FS_BLOCK_SIZE,
            );
        }
        fence(Ordering::SeqCst);
    }
    unsafe {
        PENDING_REQUESTS[request_slot] = PendingRequest::none();
    }

    if status != 0 {
        log("virtio-disk-server: ");
        log(pending.op.name());
        log(" failed status=");
        print_u64(status as u64);
        log(" block=");
        print_u64(pending.blockno);
        log("\n");
        return Some((
            request_slot,
            target,
            [XV6_EINVAL, 0, pending.blockno, status as u64],
        ));
    }

    trace_block_io(pending.op.name(), pending.blockno);
    let bytes = if pending.op == DiskOp::Flush {
        0
    } else {
        FS_BLOCK_SIZE as u64
    };
    Some((request_slot, target, [XV6_OK, bytes, pending.blockno, 0]))
}

fn ack_irq_handler() -> bool {
    let reply = unsafe {
        sel4_call(
            XV6_DISK_IRQ_HANDLER_CPTR,
            msg_info(LABEL_IRQ_ACK, 0, 0, 0),
            &[],
        )
    };
    if msg_label(reply.info) != 0 {
        log("virtio-disk-server: irq ack failed label=");
        print_u64(msg_label(reply.info));
        log("\n");
        return false;
    }
    true
}

fn ack_virtio_interrupt() {
    let irq_status = mmio_read32(VIRTIO_MMIO_INTERRUPT_STATUS) & 0x3;
    if irq_status != 0 {
        mmio_write32(VIRTIO_MMIO_INTERRUPT_ACK, irq_status);
    }
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

fn trace_completion_signal(completion_id: u64) {
    if !TRACE_BLOCK_IO {
        return;
    }
    log("virtio-disk-server: signal completion=");
    print_u64(completion_id);
    log("\n");
}

fn trace_completion_signaled(completion_id: u64) {
    if !TRACE_BLOCK_IO {
        return;
    }
    log("virtio-disk-server: signaled completion=");
    print_u64(completion_id);
    log("\n");
}

fn reply_cptr(request_slot: usize) -> u64 {
    XV6_SERVER_REPLY_CPTR + request_slot as u64
}

fn desc_head(request_slot: usize) -> u16 {
    (request_slot as u16) * DESCS_PER_REQUEST
}

fn request_slot_from_head(head: u16) -> Option<usize> {
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

fn req_off(request_slot: usize) -> u64 {
    REQUEST_AREA_OFF + request_slot as u64 * REQUEST_STRIDE + REQ_REL_OFF
}

fn data_off(request_slot: usize) -> u64 {
    REQUEST_AREA_OFF + request_slot as u64 * REQUEST_STRIDE + DATA_REL_OFF
}

fn status_off(request_slot: usize) -> u64 {
    REQUEST_AREA_OFF + request_slot as u64 * REQUEST_STRIDE + STATUS_REL_OFF
}

fn shared_buffer_va(shared_slot: u64) -> usize {
    (XV6_DISK_SHARED_BUFFER_VADDR + shared_slot * FS_BLOCK_SIZE as u64) as usize
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

fn completion_read64(offset: u64) -> u64 {
    unsafe { ptr::read_volatile((XV6_DISK_COMPLETION_RING_VADDR + offset) as *const u64) }
}

fn completion_write64(offset: u64, value: u64) {
    unsafe { ptr::write_volatile((XV6_DISK_COMPLETION_RING_VADDR + offset) as *mut u64, value) }
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
