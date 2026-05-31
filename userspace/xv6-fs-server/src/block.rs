use core::ptr;
use core::sync::atomic::{Ordering, fence};

use sel4_user::{log, msg_info, print_u64, read_u32, sel4_recv, sel4_send, write_u32};
use xv6_abi::{
    DISK_OP_FLUSH, DISK_OP_READ, DISK_OP_WRITE, FS_BLOCK_SIZE, XV6_ABI_VERSION,
    XV6_DISK_COMPLETION_BADGE, XV6_DISK_COMPLETION_ENTRY_WORDS, XV6_DISK_COMPLETION_NTFN_CPTR,
    XV6_DISK_COMPLETION_RING_ENTRIES, XV6_DISK_COMPLETION_RING_VADDR, XV6_DISK_ENDPOINT_CPTR,
    XV6_DISK_SHARED_BUFFER_VADDR, XV6_EINVAL, XV6_FS_TO_DISK_PROTOCOL, XV6_MAX_FILE_WRITE, XV6_OK,
    Xv6Superblock,
};

use crate::types::{
    BLOCK_CACHE_AGES, BLOCK_CACHE_BLOCKNOS, BLOCK_CACHE_CLOCK, BLOCK_CACHE_DATA, BLOCK_CACHE_VALID,
    FS_BLOCK_CACHE_CAP, FS_STATE, FsState, LOG_ACTIVE, LOG_BLOCKNOS, LOG_BLOCKS, LOG_LEN,
    XV6_LOG_MAX_BLOCKS,
};

const HOST_SHARED_SLOT: u64 = 0;
const DISK_SHARED_SLOT: u64 = 3;
const COMPLETION_WRITE_IDX_OFF: u64 = 0;
const COMPLETION_READ_IDX_OFF: u64 = 8;
const COMPLETION_ENTRIES_OFF: u64 = 16;
const COMPLETION_ENTRY_STRIDE: u64 = (XV6_DISK_COMPLETION_ENTRY_WORDS as u64) * 8;
const TRACE_FS_DISK: bool = option_env!("XV6_TRACE_FS_DISK").is_some();

static mut NEXT_DISK_COMPLETION_ID: u64 = 1;

pub(crate) fn handle_transactional<F>(op: F) -> [u64; 4]
where
    F: FnOnce() -> [u64; 4],
{
    if !begin_transaction() {
        return [XV6_EINVAL, 0, 0, 0];
    }
    let reply = op();
    if reply[0] != XV6_OK {
        abort_transaction();
        return reply;
    }
    if !commit_transaction() {
        abort_transaction();
        return [XV6_EINVAL, 0, 0, 0];
    }
    reply
}

fn begin_transaction() -> bool {
    unsafe {
        if LOG_ACTIVE {
            log("xv6-fs-server: nested transaction\n");
            return false;
        }
        LOG_ACTIVE = true;
        LOG_LEN = 0;
    }
    true
}

fn abort_transaction() {
    unsafe {
        LOG_LEN = 0;
        LOG_ACTIVE = false;
    }
}

fn commit_transaction() -> bool {
    let len = unsafe { LOG_LEN };
    if len == 0 {
        abort_transaction();
        return true;
    }

    let state = unsafe { FS_STATE };
    let capacity = log_capacity(&state);
    if !state.ready || len > capacity {
        return false;
    }

    let mut i = 0usize;
    while i < len {
        unsafe {
            ptr::copy_nonoverlapping(
                log_block_ptr(i) as *const u8,
                shared_block_va() as *mut u8,
                FS_BLOCK_SIZE,
            );
        }
        fence(Ordering::SeqCst);
        if !write_disk_block_raw(state.superblock.logstart + 1 + i as u32) {
            return false;
        }
        i += 1;
    }

    if !flush_disk() {
        return false;
    }
    if !write_log_header(len) {
        return false;
    }
    if !flush_disk() {
        return false;
    }

    i = 0;
    while i < len {
        unsafe {
            ptr::copy_nonoverlapping(
                log_block_ptr(i) as *const u8,
                shared_block_va() as *mut u8,
                FS_BLOCK_SIZE,
            );
        }
        fence(Ordering::SeqCst);
        if !write_disk_block_raw(log_blockno(i)) {
            return false;
        }
        i += 1;
    }

    if !flush_disk() {
        return false;
    }
    if !write_log_header(0) {
        return false;
    }
    if !flush_disk() {
        return false;
    }
    abort_transaction();
    true
}

pub(crate) fn recover_log() -> bool {
    let state = unsafe { FS_STATE };
    if !state.ready {
        return false;
    }
    let Some(len) = read_log_header() else {
        return false;
    };
    if len > log_capacity(&state) {
        log("xv6-fs-server: invalid log length=");
        print_u64(len as u64);
        log("\n");
        return false;
    }
    if len != 0 {
        log("xv6-fs-server: recovering log blocks=");
        print_u64(len as u64);
        log("\n");
    }

    let mut i = 0usize;
    while i < len {
        let dst = log_blockno(i);
        if dst >= state.superblock.size
            || !read_disk_block_raw(state.superblock.logstart + 1 + i as u32)
        {
            return false;
        }
        fence(Ordering::SeqCst);
        if !write_disk_block_raw(dst) {
            return false;
        }
        i += 1;
    }
    if !flush_disk() {
        return false;
    }
    write_log_header(0) && flush_disk()
}

fn read_log_header() -> Option<usize> {
    let state = unsafe { FS_STATE };
    if !state.ready || !read_disk_block_raw(state.superblock.logstart) {
        return None;
    }
    fence(Ordering::SeqCst);
    let block = shared_block();
    let len = read_u32(block, 0) as usize;
    if len > log_capacity(&state) {
        return None;
    }
    let mut i = 0usize;
    while i < len {
        let blockno = read_u32(block, 4 + i * 4);
        if blockno >= state.superblock.size {
            return None;
        }
        set_log_blockno(i, blockno);
        i += 1;
    }
    unsafe {
        LOG_LEN = len;
    }
    Some(len)
}

fn write_log_header(len: usize) -> bool {
    let state = unsafe { FS_STATE };
    if !state.ready || len > log_capacity(&state) {
        return false;
    }
    {
        let block = shared_block_mut();
        let mut i = 0usize;
        while i < FS_BLOCK_SIZE {
            block[i] = 0;
            i += 1;
        }
        write_u32(block, 0, len as u32);
        i = 0;
        while i < len {
            write_u32(block, 4 + i * 4, log_blockno(i));
            i += 1;
        }
    }
    fence(Ordering::SeqCst);
    write_disk_block_raw(state.superblock.logstart)
}

fn log_write_shared(blockno: u32) -> bool {
    let state = unsafe { FS_STATE };
    if !state.ready || blockno >= state.superblock.size {
        return false;
    }
    if blockno >= state.superblock.logstart
        && blockno
            < state
                .superblock
                .logstart
                .saturating_add(state.superblock.nlog)
    {
        log("xv6-fs-server: refusing to journal log block=");
        print_u64(blockno as u64);
        log("\n");
        return false;
    }

    let index = if let Some(index) = find_logged_block(blockno) {
        index
    } else {
        let len = unsafe { LOG_LEN };
        if len >= log_capacity(&state) {
            log("xv6-fs-server: transaction too large\n");
            return false;
        }
        set_log_blockno(len, blockno);
        unsafe {
            LOG_LEN = len + 1;
        }
        len
    };

    fence(Ordering::SeqCst);
    unsafe {
        ptr::copy_nonoverlapping(
            shared_block_va() as *const u8,
            log_block_ptr(index),
            FS_BLOCK_SIZE,
        );
    }
    invalidate_cached_block(blockno);
    true
}

fn find_logged_block(blockno: u32) -> Option<usize> {
    let len = unsafe { LOG_LEN };
    let mut i = 0usize;
    while i < len {
        if log_blockno(i) == blockno {
            return Some(i);
        }
        i += 1;
    }
    None
}

fn log_capacity(state: &FsState) -> usize {
    core::cmp::min(
        state.superblock.nlog.saturating_sub(1) as usize,
        XV6_LOG_MAX_BLOCKS,
    )
}

fn log_blockno(index: usize) -> u32 {
    unsafe { ptr::addr_of!(LOG_BLOCKNOS).cast::<u32>().add(index).read() }
}

fn set_log_blockno(index: usize, blockno: u32) {
    unsafe {
        ptr::addr_of_mut!(LOG_BLOCKNOS)
            .cast::<u32>()
            .add(index)
            .write(blockno);
    }
}

unsafe fn log_block_ptr(index: usize) -> *mut u8 {
    unsafe {
        ptr::addr_of_mut!(LOG_BLOCKS)
            .cast::<u8>()
            .add(index * FS_BLOCK_SIZE)
    }
}

fn load_cached_block(blockno: u32) -> bool {
    let mut i = 0usize;
    while i < FS_BLOCK_CACHE_CAP {
        let valid = unsafe {
            ptr::addr_of!(BLOCK_CACHE_VALID)
                .cast::<bool>()
                .add(i)
                .read()
        };
        let cached_blockno = unsafe {
            ptr::addr_of!(BLOCK_CACHE_BLOCKNOS)
                .cast::<u32>()
                .add(i)
                .read()
        };
        if valid && cached_blockno == blockno {
            unsafe {
                ptr::copy_nonoverlapping(
                    block_cache_data_ptr(i) as *const u8,
                    shared_block_va() as *mut u8,
                    FS_BLOCK_SIZE,
                );
            }
            touch_cached_block(i);
            fence(Ordering::SeqCst);
            return true;
        }
        i += 1;
    }
    false
}

fn store_cached_block(blockno: u32) {
    let slot = select_cache_slot(blockno);
    fence(Ordering::SeqCst);
    unsafe {
        ptr::copy_nonoverlapping(
            shared_block_va() as *const u8,
            block_cache_data_ptr(slot),
            FS_BLOCK_SIZE,
        );
        ptr::addr_of_mut!(BLOCK_CACHE_BLOCKNOS)
            .cast::<u32>()
            .add(slot)
            .write(blockno);
        ptr::addr_of_mut!(BLOCK_CACHE_VALID)
            .cast::<bool>()
            .add(slot)
            .write(true);
    }
    touch_cached_block(slot);
}

fn invalidate_cached_block(blockno: u32) {
    let mut i = 0usize;
    while i < FS_BLOCK_CACHE_CAP {
        let valid = unsafe {
            ptr::addr_of!(BLOCK_CACHE_VALID)
                .cast::<bool>()
                .add(i)
                .read()
        };
        let cached_blockno = unsafe {
            ptr::addr_of!(BLOCK_CACHE_BLOCKNOS)
                .cast::<u32>()
                .add(i)
                .read()
        };
        if valid && cached_blockno == blockno {
            unsafe {
                ptr::addr_of_mut!(BLOCK_CACHE_VALID)
                    .cast::<bool>()
                    .add(i)
                    .write(false);
            }
        }
        i += 1;
    }
}

fn select_cache_slot(blockno: u32) -> usize {
    let mut oldest_slot = 0usize;
    let mut oldest_age = u64::MAX;
    let mut i = 0usize;
    while i < FS_BLOCK_CACHE_CAP {
        let valid = unsafe {
            ptr::addr_of!(BLOCK_CACHE_VALID)
                .cast::<bool>()
                .add(i)
                .read()
        };
        let cached_blockno = unsafe {
            ptr::addr_of!(BLOCK_CACHE_BLOCKNOS)
                .cast::<u32>()
                .add(i)
                .read()
        };
        if !valid || cached_blockno == blockno {
            return i;
        }
        let age = unsafe { ptr::addr_of!(BLOCK_CACHE_AGES).cast::<u64>().add(i).read() };
        if age < oldest_age {
            oldest_age = age;
            oldest_slot = i;
        }
        i += 1;
    }
    oldest_slot
}

fn touch_cached_block(slot: usize) {
    unsafe {
        let clock = ptr::addr_of!(BLOCK_CACHE_CLOCK).read();
        let next = clock.wrapping_add(1).max(1);
        ptr::addr_of_mut!(BLOCK_CACHE_CLOCK).write(next);
        ptr::addr_of_mut!(BLOCK_CACHE_AGES)
            .cast::<u64>()
            .add(slot)
            .write(clock);
    }
}

fn block_cache_data_ptr(slot: usize) -> *mut u8 {
    unsafe {
        ptr::addr_of_mut!(BLOCK_CACHE_DATA)
            .cast::<u8>()
            .add(slot * FS_BLOCK_SIZE)
    }
}

pub(crate) fn read_disk_block(blockno: u32) -> bool {
    if unsafe { LOG_ACTIVE } {
        if let Some(index) = find_logged_block(blockno) {
            unsafe {
                ptr::copy_nonoverlapping(
                    log_block_ptr(index) as *const u8,
                    shared_block_va() as *mut u8,
                    FS_BLOCK_SIZE,
                );
            }
            fence(Ordering::SeqCst);
            return true;
        }
    }
    if load_cached_block(blockno) {
        return true;
    }
    read_disk_block_raw(blockno)
}

fn read_disk_block_raw(blockno: u32) -> bool {
    let reply = disk_data_request_and_wait(DISK_OP_READ, blockno);
    if reply[0] != XV6_OK {
        log("xv6-fs-server: disk read failed block=");
        print_u64(blockno as u64);
        log(" status=");
        print_u64(reply[0]);
        log("\n");
        return false;
    }
    let ok = reply[1] == FS_BLOCK_SIZE as u64 && reply[2] == blockno as u64;
    if ok {
        store_cached_block(blockno);
    }
    ok
}

pub(crate) fn write_disk_block(blockno: u32) -> bool {
    if unsafe { LOG_ACTIVE } {
        return log_write_shared(blockno);
    }
    write_disk_block_raw(blockno)
}

fn write_disk_block_raw(blockno: u32) -> bool {
    invalidate_cached_block(blockno);
    let reply = disk_data_request_and_wait(DISK_OP_WRITE, blockno);
    if reply[0] != XV6_OK {
        log("xv6-fs-server: disk write failed block=");
        print_u64(blockno as u64);
        log(" status=");
        print_u64(reply[0]);
        log("\n");
        return false;
    }
    let ok = reply[1] == FS_BLOCK_SIZE as u64 && reply[2] == blockno as u64;
    if ok {
        store_cached_block(blockno);
    }
    ok
}

fn flush_disk() -> bool {
    let completion_id = next_disk_completion_id();
    trace_disk_request(DISK_OP_FLUSH, 0, completion_id);
    unsafe {
        sel4_send(
            XV6_DISK_ENDPOINT_CPTR,
            msg_info(DISK_OP_FLUSH, 0, 0, 3),
            &[XV6_FS_TO_DISK_PROTOCOL, XV6_ABI_VERSION, completion_id],
        );
    }
    let reply = wait_disk_completion(completion_id);
    trace_disk_reply(DISK_OP_FLUSH, 0, completion_id, reply);
    if reply[0] != XV6_OK {
        log("xv6-fs-server: disk flush failed status=");
        print_u64(reply[0]);
        log("\n");
        return false;
    }
    true
}

fn disk_data_request_and_wait(op: u64, blockno: u32) -> [u64; 4] {
    let completion_id = next_disk_completion_id();
    trace_disk_request(op, blockno, completion_id);
    unsafe {
        sel4_send(
            XV6_DISK_ENDPOINT_CPTR,
            msg_info(op, 0, 0, 5),
            &[
                XV6_FS_TO_DISK_PROTOCOL,
                XV6_ABI_VERSION,
                blockno as u64,
                DISK_SHARED_SLOT,
                completion_id,
            ],
        );
    }
    let reply = wait_disk_completion(completion_id);
    trace_disk_reply(op, blockno, completion_id, reply);
    reply
}

fn wait_disk_completion(completion_id: u64) -> [u64; 4] {
    loop {
        match pop_disk_completion(completion_id) {
            CompletionPoll::Matched(reply) => return reply,
            CompletionPoll::Unexpected(id) => {
                log_unexpected_ring_completion(id, completion_id);
                return [XV6_EINVAL, 0, 0, 0];
            }
            CompletionPoll::Empty => {}
        }

        let msg = unsafe { sel4_recv(XV6_DISK_COMPLETION_NTFN_CPTR) };
        if (msg.badge & XV6_DISK_COMPLETION_BADGE) == 0 {
            log_unexpected_disk_notification(msg.badge, completion_id);
        }
    }
}

enum CompletionPoll {
    Empty,
    Matched([u64; 4]),
    Unexpected(u64),
}

fn pop_disk_completion(expected_id: u64) -> CompletionPoll {
    let read_idx = completion_read64(COMPLETION_READ_IDX_OFF);
    let write_idx = completion_read64(COMPLETION_WRITE_IDX_OFF);
    if read_idx == write_idx {
        return CompletionPoll::Empty;
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

    if completion_id != expected_id {
        return CompletionPoll::Unexpected(completion_id);
    }
    CompletionPoll::Matched([status, bytes, blockno, detail])
}

fn log_unexpected_ring_completion(id: u64, expected: u64) {
    log("xv6-fs-server: unexpected disk completion ring id=");
    print_u64(id);
    log(" expected=");
    print_u64(expected);
    log("\n");
}

fn log_unexpected_disk_notification(badge: u64, completion_id: u64) {
    log("xv6-fs-server: unexpected disk notification badge=");
    print_u64(badge);
    log(" expected=");
    print_u64(completion_id);
    log("\n");
}

fn next_disk_completion_id() -> u64 {
    unsafe {
        let id = NEXT_DISK_COMPLETION_ID;
        NEXT_DISK_COMPLETION_ID = NEXT_DISK_COMPLETION_ID.wrapping_add(1);
        if NEXT_DISK_COMPLETION_ID == 0 {
            NEXT_DISK_COMPLETION_ID = 1;
        }
        id
    }
}

pub(crate) fn exercise_disk_write(blockno: u32) -> bool {
    let mut backup = [0u8; FS_BLOCK_SIZE];
    if !read_disk_block(blockno) {
        return false;
    }
    fence(Ordering::SeqCst);
    unsafe {
        ptr::copy_nonoverlapping(
            shared_block_va() as *const u8,
            backup.as_mut_ptr(),
            FS_BLOCK_SIZE,
        );
        let mut i = 0usize;
        while i < FS_BLOCK_SIZE {
            let byte = (i as u8).wrapping_mul(31).wrapping_add(0xa5);
            ptr::write((shared_block_va() as *mut u8).add(i), byte);
            i += 1;
        }
    }
    fence(Ordering::SeqCst);
    let wrote_pattern = write_disk_block(blockno);
    let verified = wrote_pattern && read_disk_block(blockno) && verify_write_pattern();
    unsafe {
        ptr::copy_nonoverlapping(backup.as_ptr(), shared_block_va() as *mut u8, FS_BLOCK_SIZE);
    }
    fence(Ordering::SeqCst);
    let restored = write_disk_block(blockno);
    if verified && restored {
        log("xv6-fs-server: disk write verified block=");
        print_u64(blockno as u64);
        log("\n");
        true
    } else {
        false
    }
}

fn verify_write_pattern() -> bool {
    fence(Ordering::SeqCst);
    let block = shared_block();
    let mut i = 0usize;
    while i < FS_BLOCK_SIZE {
        let expected = (i as u8).wrapping_mul(31).wrapping_add(0xa5);
        if block[i] != expected {
            return false;
        }
        i += 1;
    }
    true
}

pub(crate) fn read_superblock_from_shared() -> Xv6Superblock {
    let block = shared_block();
    Xv6Superblock {
        magic: read_u32(block, 0),
        size: read_u32(block, 4),
        nblocks: read_u32(block, 8),
        ninodes: read_u32(block, 12),
        nlog: read_u32(block, 16),
        logstart: read_u32(block, 20),
        inodestart: read_u32(block, 24),
        bmapstart: read_u32(block, 28),
    }
}

pub(crate) fn shared_block() -> &'static [u8] {
    unsafe { core::slice::from_raw_parts(shared_block_va() as *const u8, FS_BLOCK_SIZE) }
}

pub(crate) fn shared_block_mut() -> &'static mut [u8] {
    unsafe { core::slice::from_raw_parts_mut(shared_block_va() as *mut u8, FS_BLOCK_SIZE) }
}

pub(crate) fn host_shared_write_buffer() -> &'static [u8] {
    unsafe {
        core::slice::from_raw_parts(
            shared_slot_va(HOST_SHARED_SLOT) as *const u8,
            XV6_MAX_FILE_WRITE,
        )
    }
}

pub(crate) fn host_shared_block_mut() -> &'static mut [u8] {
    unsafe {
        core::slice::from_raw_parts_mut(shared_slot_va(HOST_SHARED_SLOT) as *mut u8, FS_BLOCK_SIZE)
    }
}

fn shared_block_va() -> usize {
    shared_slot_va(DISK_SHARED_SLOT)
}

fn shared_slot_va(slot: u64) -> usize {
    (XV6_DISK_SHARED_BUFFER_VADDR + slot * FS_BLOCK_SIZE as u64) as usize
}

fn completion_read64(offset: u64) -> u64 {
    unsafe { ptr::read_volatile((XV6_DISK_COMPLETION_RING_VADDR + offset) as *const u64) }
}

fn completion_write64(offset: u64, value: u64) {
    unsafe { ptr::write_volatile((XV6_DISK_COMPLETION_RING_VADDR + offset) as *mut u64, value) }
}

fn trace_disk_request(op: u64, blockno: u32, completion_id: u64) {
    if !TRACE_FS_DISK {
        return;
    }
    log("xv6-fs-server: disk request op=");
    print_u64(op);
    log(" block=");
    print_u64(blockno as u64);
    log(" completion=");
    print_u64(completion_id);
    log("\n");
}

fn trace_disk_reply(op: u64, blockno: u32, completion_id: u64, reply: [u64; 4]) {
    if !TRACE_FS_DISK {
        return;
    }
    log("xv6-fs-server: disk reply op=");
    print_u64(op);
    log(" block=");
    print_u64(blockno as u64);
    log(" completion=");
    print_u64(completion_id);
    log(" status=");
    print_u64(reply[0]);
    log("\n");
}
