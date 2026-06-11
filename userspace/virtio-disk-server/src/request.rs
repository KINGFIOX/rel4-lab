use core::cell::UnsafeCell;

use sel4_user::{
    IpcMessage, debug, info, msg_info, msg_label, msg_len, rt, sel4_nb_recv, sel4_send, warn,
};
use xv6_abi::{
    DiskRequestOp, FS_BLOCK_SIZE, VIRTIO_BLK_SECTOR_SIZE, VIRTIO_BLK_T_IN, VIRTIO_BLK_T_OUT,
    XV6_ABI_VERSION, XV6_DISK_IRQ_NTFN_CPTR, XV6_DISK_MAX_IN_FLIGHT, XV6_DISK_SHARED_BUFFER_SLOTS,
    XV6_FS_SIZE_BLOCKS, XV6_SERVER_RECV_REPLY_CPTR, Xv6Badge, Xv6Protocol, Xv6Status,
};

use crate::completion;
use crate::device;
use crate::layout::request_slot_from_head;
use crate::types::{DiskOp, InFlightRequest, ReplyTarget, RequestResult};

const REQUEST_SLOT: usize = 0;

struct PendingRequests {
    requests: UnsafeCell<[InFlightRequest; XV6_DISK_MAX_IN_FLIGHT]>,
}

// The disk server runs this table from a single async event loop. IRQ handling
// is polled through the same loop, so table mutation is serialized by control
// flow rather than by a separate lock.
unsafe impl Sync for PendingRequests {}

impl PendingRequests {
    const fn new() -> Self {
        Self {
            requests: UnsafeCell::new([InFlightRequest::none(); XV6_DISK_MAX_IN_FLIGHT]),
        }
    }

    fn reset(&self) {
        unsafe {
            *self.requests.get() = [InFlightRequest::none(); XV6_DISK_MAX_IN_FLIGHT];
        }
    }

    fn get(&self, slot: usize) -> InFlightRequest {
        unsafe { (&*self.requests.get())[slot] }
    }

    fn set(&self, slot: usize, request: InFlightRequest) {
        unsafe {
            (&mut *self.requests.get())[slot] = request;
        }
    }

    fn is_active(&self, slot: usize) -> bool {
        self.get(slot).active
    }

    fn mark_completed(&self, slot: usize, reply: [u64; 4]) {
        unsafe {
            let requests = &mut *self.requests.get();
            requests[slot].reply = reply;
            requests[slot].completed = true;
        }
    }
}

static PENDING_REQUESTS: PendingRequests = PendingRequests::new();

pub fn init() {
    PENDING_REQUESTS.reset();
}

pub async fn handle(msg: &IpcMessage) -> RequestResult {
    let raw_op = msg_label(msg.info);
    match DiskRequestOp::from_raw(raw_op) {
        Some(DiskRequestOp::GetInfo) => RequestResult::Reply(handle_get_info(msg)),
        Some(DiskRequestOp::Read) => handle_read(msg).await,
        Some(DiskRequestOp::Write) => handle_write(msg).await,
        Some(DiskRequestOp::Flush) => handle_flush(msg).await,
        Some(DiskRequestOp::Complete) | None => {
            warn!("virtio-disk-server: unsupported op={}", raw_op);
            RequestResult::Reply([Xv6Status::NoSyscall.raw(), 0, 0, 0])
        }
    }
}

pub fn is_disk_irq(msg: &IpcMessage) -> bool {
    msg_label(msg.info) == 0 && (msg.badge & Xv6Badge::DiskIrq.raw()) != 0
}

pub fn handle_disk_irq() {
    device::ack_virtio_interrupt();
    let _ = device::ack_irq_handler();
}

fn handle_get_info(msg: &IpcMessage) -> [u64; 4] {
    if msg.mrs[0] != Xv6Protocol::FsToDisk.raw() || msg.mrs[1] != XV6_ABI_VERSION {
        warn!("virtio-disk-server: bad get-info protocol");
        return [Xv6Status::InvalidArgument.raw(), 0, 0, 0];
    }
    if !device::ready() {
        warn!("virtio-disk-server: get-info before ready");
        return [Xv6Status::InvalidArgument.raw(), 0, 0, 0];
    }
    info!("virtio-disk-server: get-info ready");
    [
        Xv6Status::Ok.raw(),
        VIRTIO_BLK_SECTOR_SIZE as u64,
        XV6_FS_SIZE_BLOCKS as u64,
        0,
    ]
}

async fn handle_read(msg: &IpcMessage) -> RequestResult {
    let target = data_reply_target(msg);
    if msg.mrs[0] != Xv6Protocol::FsToDisk.raw() || msg.mrs[1] != XV6_ABI_VERSION {
        warn!("virtio-disk-server: bad read protocol");
        return immediate_reply(target, [Xv6Status::InvalidArgument.raw(), 0, 0, 0]);
    }
    if !device::ready() {
        warn!("virtio-disk-server: read before ready");
        return immediate_reply(target, [Xv6Status::InvalidArgument.raw(), 0, 0, 0]);
    }
    let blockno = msg.mrs[2];
    if blockno >= XV6_FS_SIZE_BLOCKS as u64 {
        warn!("virtio-disk-server: read out of range block={}", blockno);
        return immediate_reply(target, [Xv6Status::InvalidArgument.raw(), 0, 0, 0]);
    }
    let Some(shared_slot) = parse_shared_slot(msg) else {
        return immediate_reply(target, [Xv6Status::InvalidArgument.raw(), 0, blockno, 0]);
    };
    if shared_slot_in_use(shared_slot) {
        reject_busy_shared_slot("read", shared_slot, blockno);
        return immediate_reply(target, [Xv6Status::InvalidArgument.raw(), 0, blockno, 0]);
    }
    let Some(request_slot) = alloc_request_slot() else {
        reject_no_request_slot("read", blockno);
        return immediate_reply(target, [Xv6Status::InvalidArgument.raw(), 0, blockno, 0]);
    };
    if !submit_block_request(
        request_slot,
        blockno,
        shared_slot,
        VIRTIO_BLK_T_IN,
        true,
        DiskOp::Read,
    ) {
        send_deferred_reply_target(
            request_slot,
            target,
            [Xv6Status::InvalidArgument.raw(), 0, blockno, 0],
        );
        return RequestResult::Deferred;
    }
    let reply = wait_for_request_completion(request_slot).await;
    send_deferred_reply_target(request_slot, target, reply);
    RequestResult::Deferred
}

async fn handle_write(msg: &IpcMessage) -> RequestResult {
    let target = data_reply_target(msg);
    if msg.mrs[0] != Xv6Protocol::FsToDisk.raw() || msg.mrs[1] != XV6_ABI_VERSION {
        warn!("virtio-disk-server: bad write protocol");
        return immediate_reply(target, [Xv6Status::InvalidArgument.raw(), 0, 0, 0]);
    }
    if !device::ready() {
        warn!("virtio-disk-server: write before ready");
        return immediate_reply(target, [Xv6Status::InvalidArgument.raw(), 0, 0, 0]);
    }
    let blockno = msg.mrs[2];
    if blockno >= XV6_FS_SIZE_BLOCKS as u64 {
        warn!("virtio-disk-server: write out of range block={}", blockno);
        return immediate_reply(target, [Xv6Status::InvalidArgument.raw(), 0, 0, 0]);
    }
    let Some(shared_slot) = parse_shared_slot(msg) else {
        return immediate_reply(target, [Xv6Status::InvalidArgument.raw(), 0, blockno, 0]);
    };
    if shared_slot_in_use(shared_slot) {
        reject_busy_shared_slot("write", shared_slot, blockno);
        return immediate_reply(target, [Xv6Status::InvalidArgument.raw(), 0, blockno, 0]);
    }
    let Some(request_slot) = alloc_request_slot() else {
        reject_no_request_slot("write", blockno);
        return immediate_reply(target, [Xv6Status::InvalidArgument.raw(), 0, blockno, 0]);
    };
    device::copy_shared_to_dma(request_slot, shared_slot);
    if !submit_block_request(
        request_slot,
        blockno,
        shared_slot,
        VIRTIO_BLK_T_OUT,
        false,
        DiskOp::Write,
    ) {
        send_deferred_reply_target(
            request_slot,
            target,
            [Xv6Status::InvalidArgument.raw(), 0, blockno, 0],
        );
        return RequestResult::Deferred;
    }
    let reply = wait_for_request_completion(request_slot).await;
    send_deferred_reply_target(request_slot, target, reply);
    RequestResult::Deferred
}

async fn handle_flush(msg: &IpcMessage) -> RequestResult {
    let target = flush_reply_target(msg);
    if msg.mrs[0] != Xv6Protocol::FsToDisk.raw() || msg.mrs[1] != XV6_ABI_VERSION {
        warn!("virtio-disk-server: bad flush protocol");
        return immediate_reply(target, [Xv6Status::InvalidArgument.raw(), 0, 0, 0]);
    }
    if !device::ready() {
        warn!("virtio-disk-server: flush before ready");
        return immediate_reply(target, [Xv6Status::InvalidArgument.raw(), 0, 0, 0]);
    }
    if any_request_in_flight() {
        warn!("virtio-disk-server: flush rejected while request in flight");
        return immediate_reply(target, [Xv6Status::InvalidArgument.raw(), 0, 0, 0]);
    }
    if !device::can_flush() {
        return immediate_reply(target, [Xv6Status::Ok.raw(), 0, 0, 0]);
    }

    let Some(request_slot) = alloc_request_slot() else {
        reject_no_request_slot("flush", 0);
        return immediate_reply(target, [Xv6Status::InvalidArgument.raw(), 0, 0, 0]);
    };
    if !submit_flush_request(request_slot) {
        send_deferred_reply_target(
            request_slot,
            target,
            [Xv6Status::InvalidArgument.raw(), 0, 0, 0],
        );
        return RequestResult::Deferred;
    }
    let reply = wait_for_request_completion(request_slot).await;
    send_deferred_reply_target(request_slot, target, reply);
    RequestResult::Deferred
}

fn any_request_in_flight() -> bool {
    let mut i = 0usize;
    while i < XV6_DISK_MAX_IN_FLIGHT {
        if PENDING_REQUESTS.is_active(i) {
            return true;
        }
        i += 1;
    }
    false
}

fn alloc_request_slot() -> Option<usize> {
    if any_request_in_flight() {
        None
    } else {
        Some(REQUEST_SLOT)
    }
}

fn shared_slot_in_use(shared_slot: u64) -> bool {
    let mut i = 0usize;
    while i < XV6_DISK_MAX_IN_FLIGHT {
        let pending = PENDING_REQUESTS.get(i);
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
        warn!("virtio-disk-server: bad shared slot={}", shared_slot);
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
        completion::send(target.completion_id, reply_mrs);
        RequestResult::Deferred
    } else {
        RequestResult::Reply(reply_mrs)
    }
}

fn reject_busy_shared_slot(op: &str, shared_slot: u64, blockno: u64) {
    warn!(
        "virtio-disk-server: {} shared slot busy slot={} block={}",
        op, shared_slot, blockno
    );
}

fn reject_no_request_slot(op: &str, blockno: u64) {
    warn!(
        "virtio-disk-server: {} request slots exhausted block={}",
        op, blockno
    );
}

fn send_deferred_reply(reply_mrs: [u64; 4]) {
    unsafe {
        sel4_send(
            XV6_SERVER_RECV_REPLY_CPTR,
            msg_info(0, 0, 0, reply_mrs.len() as u64),
            &reply_mrs,
        );
    }
}

fn send_deferred_reply_target(request_slot: usize, target: ReplyTarget, reply_mrs: [u64; 4]) {
    if target.async_completion {
        completion::send(target.completion_id, reply_mrs);
    } else {
        let _ = request_slot;
        send_deferred_reply(reply_mrs);
    }
}

fn submit_block_request(
    request_slot: usize,
    blockno: u64,
    shared_slot: u64,
    request_type: u32,
    data_writable_by_device: bool,
    op: DiskOp,
) -> bool {
    if PENDING_REQUESTS.is_active(request_slot) {
        reject_no_request_slot(op.name(), blockno);
        return false;
    }

    device::prepare_block_descriptor(request_slot, blockno, request_type, data_writable_by_device);

    // Concurrency boundary: after avail.idx is visible the device may DMA and
    // interrupt independently of this server thread. Publish the complete
    // pending state before handing descriptor ownership to the device, so IRQ
    // completion can always resolve the used-ring head back to a request slot.
    PENDING_REQUESTS.set(
        request_slot,
        InFlightRequest {
            active: true,
            op,
            blockno,
            shared_slot,
            completed: false,
            reply: [0; 4],
        },
    );

    device::publish_request(request_slot);
    true
}

fn submit_flush_request(request_slot: usize) -> bool {
    if PENDING_REQUESTS.is_active(request_slot) {
        reject_no_request_slot("flush", 0);
        return false;
    }

    device::prepare_flush_descriptor(request_slot);

    // Same ordering rule as read/write: pending state must exist before the
    // descriptor head can appear in the avail ring.
    PENDING_REQUESTS.set(
        request_slot,
        InFlightRequest {
            active: true,
            op: DiskOp::Flush,
            blockno: 0,
            shared_slot: 0,
            completed: false,
            reply: [0; 4],
        },
    );

    device::publish_request(request_slot);
    true
}

fn complete_used_requests() -> bool {
    device::begin_used_drain();
    let mut completed = false;
    while let Some(head) = device::next_used_head() {
        if complete_request_by_head(head) {
            completed = true;
        } else {
            warn!("virtio-disk-server: used entry for unknown head={}", head);
        }
    }
    device::end_used_drain();
    completed
}

fn complete_request_by_head(head: u16) -> bool {
    let Some(request_slot) = request_slot_from_head(head) else {
        return false;
    };
    let pending = PENDING_REQUESTS.get(request_slot);
    if !pending.active || pending.completed {
        return false;
    }

    let status = device::request_status(request_slot);
    if status == 0 && pending.op == DiskOp::Read {
        device::copy_dma_to_shared(request_slot, pending.shared_slot);
    }
    // Keep the request/shared slot owned by the device/server until any final
    // DMA-to-shared-buffer copy is complete. Only then can a later RPC reuse the
    // slot without racing the visible read result.

    let reply = if status != 0 {
        warn!(
            "virtio-disk-server: {} failed status={} block={}",
            pending.op.name(),
            status,
            pending.blockno
        );
        [
            Xv6Status::InvalidArgument.raw(),
            0,
            pending.blockno,
            status as u64,
        ]
    } else {
        trace_block_io(pending.op.name(), pending.blockno);
        let bytes = if pending.op == DiskOp::Flush {
            0
        } else {
            FS_BLOCK_SIZE as u64
        };
        [Xv6Status::Ok.raw(), bytes, pending.blockno, 0]
    };

    PENDING_REQUESTS.mark_completed(request_slot, reply);
    true
}

fn take_completed_request(request_slot: usize) -> Option<[u64; 4]> {
    if request_slot >= XV6_DISK_MAX_IN_FLIGHT {
        return Some([Xv6Status::InvalidArgument.raw(), 0, 0, 0]);
    }
    let pending = PENDING_REQUESTS.get(request_slot);
    if !pending.active {
        return Some([Xv6Status::InvalidArgument.raw(), 0, 0, 0]);
    }
    if !pending.completed {
        return None;
    }

    PENDING_REQUESTS.set(request_slot, InFlightRequest::none());
    Some(pending.reply)
}

async fn wait_for_request_completion(request_slot: usize) -> [u64; 4] {
    loop {
        if complete_used_requests() {
            if let Some(reply) = take_completed_request(request_slot) {
                return reply;
            }
        }

        let msg = unsafe { sel4_nb_recv(XV6_DISK_IRQ_NTFN_CPTR) };
        if msg.badge == 0 {
            rt::yield_now().await;
            continue;
        }
        if is_disk_irq(&msg) {
            handle_disk_irq();
            if complete_used_requests() {
                if let Some(reply) = take_completed_request(request_slot) {
                    return reply;
                }
            }
            continue;
        }
        warn!(
            "virtio-disk-server: unexpected notification while waiting for disk irq badge={:#x}",
            msg.badge
        );
    }
}

fn trace_block_io(op: &str, blockno: u64) {
    debug!("virtio-disk-server: {} block={}", op, blockno);
}
