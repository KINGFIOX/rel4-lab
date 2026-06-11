use core::cell::UnsafeCell;
use core::ptr;
use core::sync::atomic::{AtomicU64, Ordering, fence};

use sel4_user::{debug, msg_info, rt, sel4_call, sel4_recv, sel4_send, warn};
use xv6_abi::{
    DiskRequestOp, FS_BLOCK_SIZE, XV6_ABI_VERSION, XV6_DISK_COMPLETION_ENTRY_WORDS,
    XV6_DISK_COMPLETION_NTFN_CPTR, XV6_DISK_COMPLETION_RING_ENTRIES,
    XV6_DISK_COMPLETION_RING_VADDR, XV6_DISK_ENDPOINT_CPTR, XV6_DISK_MAX_IN_FLIGHT,
    XV6_DISK_SHARED_BUFFER_VADDR, XV6_MAX_FILE_WRITE, XV6_XV6FS_SCRATCH_SLOT_BASE,
    XV6_XV6FS_SCRATCH_SLOT_COUNT, Xv6Badge, Xv6Protocol, Xv6Status,
};

const HOST_SHARED_SLOT: u64 = 0;
const DISK_SHARED_SLOT: u64 = 3;
const CHECK_SHARED_SLOT: u64 = 2;
const COMPLETION_WRITE_IDX_OFF: u64 = 0;
const COMPLETION_READ_IDX_OFF: u64 = 8;
const COMPLETION_ENTRIES_OFF: u64 = 16;
const COMPLETION_ENTRY_STRIDE: u64 = (XV6_DISK_COMPLETION_ENTRY_WORDS as u64) * 8;

struct DiskRuntime {
    next_completion_id: AtomicU64,
    completion_stash: UnsafeCell<[StashedCompletion; XV6_DISK_MAX_IN_FLIGHT]>,
    scratch_slots: UnsafeCell<[bool; XV6_XV6FS_SCRATCH_SLOT_COUNT]>,
}

// xv6fs-server runs a single request future at a time. Disk completions and
// scratch-slot reuse are polled through that same cooperative control flow.
unsafe impl Sync for DiskRuntime {}

impl DiskRuntime {
    const fn new() -> Self {
        Self {
            next_completion_id: AtomicU64::new(1),
            completion_stash: UnsafeCell::new([StashedCompletion::empty(); XV6_DISK_MAX_IN_FLIGHT]),
            scratch_slots: UnsafeCell::new([false; XV6_XV6FS_SCRATCH_SLOT_COUNT]),
        }
    }

    fn next_completion_id(&self) -> u64 {
        loop {
            let id = self.next_completion_id.load(Ordering::Relaxed);
            let next_id = match id.wrapping_add(1) {
                0 => 1,
                id => id,
            };
            if self
                .next_completion_id
                .compare_exchange_weak(id, next_id, Ordering::Relaxed, Ordering::Relaxed)
                .is_ok()
            {
                return id;
            }
        }
    }

    fn take_stashed_completion(&self, completion_id: u64) -> Option<[u64; 4]> {
        let stash = unsafe { &mut *self.completion_stash.get() };
        let mut i = 0usize;
        while i < XV6_DISK_MAX_IN_FLIGHT {
            let stashed = stash[i];
            if stashed.valid && stashed.completion_id == completion_id {
                stash[i] = StashedCompletion::empty();
                return Some(stashed.reply);
            }
            i += 1;
        }
        None
    }

    fn stash_completion(&self, completion: DiskCompletion) -> bool {
        let stash = unsafe { &mut *self.completion_stash.get() };
        let mut i = 0usize;
        while i < XV6_DISK_MAX_IN_FLIGHT {
            if !stash[i].valid {
                stash[i] = StashedCompletion {
                    valid: true,
                    completion_id: completion.completion_id,
                    reply: completion.reply,
                };
                return true;
            }
            i += 1;
        }
        warn!(
            "xv6fs-server: disk completion stash full id={}",
            completion.completion_id
        );
        false
    }

    fn alloc_scratch_slot(&self) -> Option<ScratchSlot> {
        let scratch_slots = unsafe { &mut *self.scratch_slots.get() };
        let mut i = 0usize;
        while i < XV6_XV6FS_SCRATCH_SLOT_COUNT {
            if !scratch_slots[i] {
                scratch_slots[i] = true;
                return Some(ScratchSlot {
                    slot: (XV6_XV6FS_SCRATCH_SLOT_BASE + i) as u64,
                });
            }
            i += 1;
        }
        None
    }

    fn free_scratch_slot(&self, slot: u64) {
        let base = XV6_XV6FS_SCRATCH_SLOT_BASE as u64;
        let count = XV6_XV6FS_SCRATCH_SLOT_COUNT as u64;
        if slot < base || slot >= base + count {
            return;
        }
        let scratch_slots = unsafe { &mut *self.scratch_slots.get() };
        scratch_slots[(slot - base) as usize] = false;
    }
}

static DISK_RUNTIME: DiskRuntime = DiskRuntime::new();

#[derive(Copy, Clone)]
struct StashedCompletion {
    valid: bool,
    completion_id: u64,
    reply: [u64; 4],
}

impl StashedCompletion {
    const fn empty() -> Self {
        Self {
            valid: false,
            completion_id: 0,
            reply: [0; 4],
        }
    }
}

pub(crate) fn disk_data_request_and_wait(op: u64, blockno: u32) -> [u64; 4] {
    let reply = unsafe {
        sel4_call(
            XV6_DISK_ENDPOINT_CPTR,
            msg_info(op, 0, 0, 4),
            &[
                Xv6Protocol::FsToDisk.raw(),
                XV6_ABI_VERSION,
                blockno as u64,
                DiskSharedSlot::Main.value(),
            ],
        )
    };
    [reply.mrs[0], reply.mrs[1], reply.mrs[2], reply.mrs[3]]
}

pub(crate) fn exercise_concurrent_reads(block_a: u32, block_b: u32) -> bool {
    let first = submit_disk_data_request(DiskRequestOp::Read.raw(), block_a, DiskSharedSlot::Check);
    let second = submit_disk_data_request(DiskRequestOp::Read.raw(), block_b, DiskSharedSlot::Main);

    let second_reply = wait_disk_request(second);
    let first_reply = wait_disk_request(first);

    disk_reply_matches(second_reply, block_b) && disk_reply_matches(first_reply, block_a)
}

#[derive(Copy, Clone)]
pub(crate) struct DiskRequest {
    op: u64,
    blockno: u32,
    completion_id: u64,
}

#[derive(Copy, Clone)]
pub(crate) enum DiskSharedSlot {
    Main,
    Check,
}

impl DiskSharedSlot {
    pub(crate) const fn value(self) -> u64 {
        match self {
            Self::Main => DISK_SHARED_SLOT,
            Self::Check => CHECK_SHARED_SLOT,
        }
    }
}

pub(crate) fn submit_disk_data_request(
    op: u64,
    blockno: u32,
    shared_slot: DiskSharedSlot,
) -> DiskRequest {
    submit_disk_data_request_raw(op, blockno, shared_slot.value())
}

pub(crate) fn submit_disk_data_request_raw(op: u64, blockno: u32, shared_slot: u64) -> DiskRequest {
    let completion_id = next_disk_completion_id();
    trace_disk_request(op, blockno, completion_id);
    unsafe {
        sel4_send(
            XV6_DISK_ENDPOINT_CPTR,
            msg_info(op, 0, 0, 5),
            &[
                Xv6Protocol::FsToDisk.raw(),
                XV6_ABI_VERSION,
                blockno as u64,
                shared_slot,
                completion_id,
            ],
        );
    }
    DiskRequest {
        op,
        blockno,
        completion_id,
    }
}

pub(crate) async fn disk_data_request(op: u64, blockno: u32, shared_slot: u64) -> [u64; 4] {
    let request = submit_disk_data_request_raw(op, blockno, shared_slot);
    wait_disk_request_async(request).await
}

pub(crate) fn wait_disk_request(request: DiskRequest) -> [u64; 4] {
    let reply = loop {
        match poll_disk_request(request) {
            DiskPoll::Ready(reply) => break reply,
            DiskPoll::Pending => wait_disk_notification(request),
            DiskPoll::Error => break [Xv6Status::InvalidArgument.raw(), 0, 0, 0],
        }
    };
    trace_disk_reply(request.op, request.blockno, request.completion_id, reply);
    reply
}

pub(crate) enum DiskPoll {
    Pending,
    Ready([u64; 4]),
    Error,
}

pub(crate) fn poll_disk_request(request: DiskRequest) -> DiskPoll {
    if let Some(reply) = take_stashed_completion(request.completion_id) {
        return DiskPoll::Ready(reply);
    }

    while let Some(completion) = pop_disk_completion() {
        if completion.completion_id == request.completion_id {
            return DiskPoll::Ready(completion.reply);
        }
        if !stash_completion(completion) {
            return DiskPoll::Error;
        }
    }
    DiskPoll::Pending
}

fn disk_reply_matches(reply: [u64; 4], blockno: u32) -> bool {
    reply[0] == Xv6Status::Ok.raw()
        && reply[1] == FS_BLOCK_SIZE as u64
        && reply[2] == blockno as u64
}

pub(crate) fn flush_disk() -> bool {
    let request = submit_flush_request();
    let reply = wait_disk_request(request);
    if reply[0] != Xv6Status::Ok.raw() {
        warn!("xv6fs-server: disk flush failed status={}", reply[0]);
        return false;
    }
    true
}

pub(crate) async fn flush_disk_async() -> bool {
    let mut retries = 0usize;
    loop {
        let request = submit_flush_request();
        let reply = wait_disk_request_async(request).await;
        if reply[0] == Xv6Status::Ok.raw() {
            return true;
        }
        retries += 1;
        if retries >= 1024 {
            warn!("xv6fs-server: disk flush failed status={}", reply[0]);
            return false;
        }
        rt::yield_now().await;
    }
}

pub(crate) async fn wait_disk_request_async(request: DiskRequest) -> [u64; 4] {
    loop {
        match poll_disk_request(request) {
            DiskPoll::Ready(reply) => {
                trace_disk_reply(request.op, request.blockno, request.completion_id, reply);
                return reply;
            }
            DiskPoll::Error => return [Xv6Status::InvalidArgument.raw(), 0, 0, 0],
            DiskPoll::Pending => wait_disk_notification_async(request).await,
        }
    }
}

pub(crate) fn submit_flush_request() -> DiskRequest {
    let completion_id = next_disk_completion_id();
    trace_disk_request(DiskRequestOp::Flush.raw(), 0, completion_id);
    unsafe {
        sel4_send(
            XV6_DISK_ENDPOINT_CPTR,
            msg_info(DiskRequestOp::Flush.raw(), 0, 0, 3),
            &[Xv6Protocol::FsToDisk.raw(), XV6_ABI_VERSION, completion_id],
        );
    }
    DiskRequest {
        op: DiskRequestOp::Flush.raw(),
        blockno: 0,
        completion_id,
    }
}

fn wait_disk_notification(request: DiskRequest) {
    let msg = unsafe { sel4_recv(XV6_DISK_COMPLETION_NTFN_CPTR) };
    if (msg.badge & Xv6Badge::DiskCompletion.raw()) == 0 {
        log_unexpected_disk_notification(msg.badge, request.completion_id);
    }
}

async fn wait_disk_notification_async(request: DiskRequest) {
    let msg = rt::recv(XV6_DISK_COMPLETION_NTFN_CPTR).await;
    if (msg.badge & Xv6Badge::DiskCompletion.raw()) == 0 {
        log_unexpected_disk_notification(msg.badge, request.completion_id);
    }
}

#[derive(Copy, Clone)]
struct DiskCompletion {
    completion_id: u64,
    reply: [u64; 4],
}

fn pop_disk_completion() -> Option<DiskCompletion> {
    let read_idx = completion_read64(COMPLETION_READ_IDX_OFF);
    let write_idx = completion_read64(COMPLETION_WRITE_IDX_OFF);
    if read_idx == write_idx {
        return None;
    }

    fence(Ordering::SeqCst);
    let slot = read_idx % XV6_DISK_COMPLETION_RING_ENTRIES as u64;
    let entry = COMPLETION_ENTRIES_OFF + slot * COMPLETION_ENTRY_STRIDE;
    let status = completion_read64(entry);
    let bytes = completion_read64(entry + 8);
    let blockno = completion_read64(entry + 16);
    let completion_id = completion_read64(entry + 24);
    let detail = completion_read64(entry + 32);
    fence(Ordering::SeqCst);
    completion_write64(COMPLETION_READ_IDX_OFF, read_idx.wrapping_add(1));

    Some(DiskCompletion {
        completion_id,
        reply: [status, bytes, blockno, detail],
    })
}

fn take_stashed_completion(completion_id: u64) -> Option<[u64; 4]> {
    DISK_RUNTIME.take_stashed_completion(completion_id)
}

fn stash_completion(completion: DiskCompletion) -> bool {
    DISK_RUNTIME.stash_completion(completion)
}

fn log_unexpected_disk_notification(badge: u64, completion_id: u64) {
    warn!(
        "xv6fs-server: unexpected disk notification badge={} expected={}",
        badge, completion_id
    );
}

fn next_disk_completion_id() -> u64 {
    DISK_RUNTIME.next_completion_id()
}

pub(crate) fn shared_block() -> &'static [u8] {
    unsafe { core::slice::from_raw_parts(shared_block_ptr() as *const u8, FS_BLOCK_SIZE) }
}

pub(crate) fn with_shared_block_mut<R>(op: impl FnOnce(&mut [u8]) -> R) -> R {
    let block = unsafe { core::slice::from_raw_parts_mut(shared_block_ptr(), FS_BLOCK_SIZE) };
    op(block)
}

pub(crate) fn copy_shared_block_from_ptr(src: *const u8) {
    unsafe {
        ptr::copy_nonoverlapping(src, shared_block_ptr(), FS_BLOCK_SIZE);
    }
}

pub(crate) fn copy_shared_block_to_ptr(dst: *mut u8) {
    unsafe {
        ptr::copy_nonoverlapping(shared_block_ptr() as *const u8, dst, FS_BLOCK_SIZE);
    }
}

pub(crate) fn host_shared_write_buffer() -> &'static [u8] {
    unsafe {
        core::slice::from_raw_parts(
            shared_slot_ptr(HOST_SHARED_SLOT) as *const u8,
            XV6_MAX_FILE_WRITE,
        )
    }
}

pub(crate) fn copy_host_shared_block_from(src: &[u8]) {
    let len = core::cmp::min(src.len(), FS_BLOCK_SIZE);
    let dst = unsafe {
        core::slice::from_raw_parts_mut(shared_slot_ptr(HOST_SHARED_SLOT), FS_BLOCK_SIZE)
    };
    dst[..len].copy_from_slice(&src[..len]);
}

fn shared_slot_block(slot: u64) -> &'static [u8] {
    unsafe { core::slice::from_raw_parts(shared_slot_ptr(slot) as *const u8, FS_BLOCK_SIZE) }
}

fn shared_block_ptr() -> *mut u8 {
    shared_slot_ptr(DISK_SHARED_SLOT)
}

fn shared_slot_ptr(slot: u64) -> *mut u8 {
    (XV6_DISK_SHARED_BUFFER_VADDR + slot * FS_BLOCK_SIZE as u64) as *mut u8
}

pub(crate) struct ScratchSlot {
    slot: u64,
}

impl ScratchSlot {
    pub(crate) fn value(&self) -> u64 {
        self.slot
    }

    pub(crate) fn with_block<R>(&self, op: impl FnOnce(&[u8]) -> R) -> R {
        op(shared_slot_block(self.slot))
    }
}

impl Drop for ScratchSlot {
    fn drop(&mut self) {
        free_scratch_slot(self.slot);
    }
}

pub(crate) fn alloc_scratch_slot() -> Option<ScratchSlot> {
    DISK_RUNTIME.alloc_scratch_slot()
}

pub(crate) async fn alloc_scratch_slot_async() -> ScratchSlot {
    loop {
        if let Some(scratch) = alloc_scratch_slot() {
            return scratch;
        }
        rt::yield_now().await;
    }
}

fn free_scratch_slot(slot: u64) {
    DISK_RUNTIME.free_scratch_slot(slot);
}

fn completion_read64(offset: u64) -> u64 {
    unsafe { ptr::read_volatile((XV6_DISK_COMPLETION_RING_VADDR + offset) as *const u64) }
}

fn completion_write64(offset: u64, value: u64) {
    unsafe { ptr::write_volatile((XV6_DISK_COMPLETION_RING_VADDR + offset) as *mut u64, value) }
}

fn trace_disk_request(op: u64, blockno: u32, completion_id: u64) {
    debug!(
        "xv6fs-server: disk request op={} block={} completion={}",
        op, blockno, completion_id
    );
}

fn trace_disk_reply(op: u64, blockno: u32, completion_id: u64, reply: [u64; 4]) {
    debug!(
        "xv6fs-server: disk reply op={} block={} completion={} status={}",
        op, blockno, completion_id, reply[0]
    );
}
