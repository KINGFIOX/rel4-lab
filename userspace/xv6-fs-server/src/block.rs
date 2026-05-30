use core::ptr;
use core::sync::atomic::{Ordering, fence};

use sel4_user::{log, msg_info, msg_label, print_u64, read_u32, sel4_call, write_u32};
use xv6_abi::{
    DISK_OP_READ, DISK_OP_WRITE, FS_BLOCK_SIZE, XV6_ABI_VERSION, XV6_DISK_ENDPOINT_CPTR,
    XV6_DISK_SHARED_BUFFER_VADDR, XV6_EINVAL, XV6_FS_TO_DISK_PROTOCOL, XV6_OK, Xv6Superblock,
};

use crate::types::{
    FS_STATE, FsState, LOG_ACTIVE, LOG_BLOCKNOS, LOG_BLOCKS, LOG_LEN, XV6_LOG_MAX_BLOCKS,
};

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
                XV6_DISK_SHARED_BUFFER_VADDR as *mut u8,
                FS_BLOCK_SIZE,
            );
        }
        fence(Ordering::SeqCst);
        if !write_disk_block_raw(state.superblock.logstart + 1 + i as u32) {
            return false;
        }
        i += 1;
    }

    if !write_log_header(len) {
        return false;
    }

    i = 0;
    while i < len {
        unsafe {
            ptr::copy_nonoverlapping(
                log_block_ptr(i) as *const u8,
                XV6_DISK_SHARED_BUFFER_VADDR as *mut u8,
                FS_BLOCK_SIZE,
            );
        }
        fence(Ordering::SeqCst);
        if !write_disk_block_raw(log_blockno(i)) {
            return false;
        }
        i += 1;
    }

    if !write_log_header(0) {
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
    write_log_header(0)
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
            XV6_DISK_SHARED_BUFFER_VADDR as *const u8,
            log_block_ptr(index),
            FS_BLOCK_SIZE,
        );
    }
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

pub(crate) fn read_disk_block(blockno: u32) -> bool {
    if unsafe { LOG_ACTIVE } {
        if let Some(index) = find_logged_block(blockno) {
            unsafe {
                ptr::copy_nonoverlapping(
                    log_block_ptr(index) as *const u8,
                    XV6_DISK_SHARED_BUFFER_VADDR as *mut u8,
                    FS_BLOCK_SIZE,
                );
            }
            fence(Ordering::SeqCst);
            return true;
        }
    }
    read_disk_block_raw(blockno)
}

fn read_disk_block_raw(blockno: u32) -> bool {
    let reply = unsafe {
        sel4_call(
            XV6_DISK_ENDPOINT_CPTR,
            msg_info(DISK_OP_READ, 0, 0, 3),
            &[XV6_FS_TO_DISK_PROTOCOL, XV6_ABI_VERSION, blockno as u64],
        )
    };
    if msg_label(reply.info) != 0 || reply.mrs[0] != XV6_OK {
        log("xv6-fs-server: disk read failed block=");
        print_u64(blockno as u64);
        log(" status=");
        print_u64(reply.mrs[0]);
        log("\n");
        return false;
    }
    reply.mrs[1] == FS_BLOCK_SIZE as u64 && reply.mrs[2] == blockno as u64
}

pub(crate) fn write_disk_block(blockno: u32) -> bool {
    if unsafe { LOG_ACTIVE } {
        return log_write_shared(blockno);
    }
    write_disk_block_raw(blockno)
}

fn write_disk_block_raw(blockno: u32) -> bool {
    let reply = unsafe {
        sel4_call(
            XV6_DISK_ENDPOINT_CPTR,
            msg_info(DISK_OP_WRITE, 0, 0, 3),
            &[XV6_FS_TO_DISK_PROTOCOL, XV6_ABI_VERSION, blockno as u64],
        )
    };
    if msg_label(reply.info) != 0 || reply.mrs[0] != XV6_OK {
        log("xv6-fs-server: disk write failed block=");
        print_u64(blockno as u64);
        log(" status=");
        print_u64(reply.mrs[0]);
        log("\n");
        return false;
    }
    reply.mrs[1] == FS_BLOCK_SIZE as u64 && reply.mrs[2] == blockno as u64
}

pub(crate) fn exercise_disk_write(blockno: u32) -> bool {
    let mut backup = [0u8; FS_BLOCK_SIZE];
    if !read_disk_block(blockno) {
        return false;
    }
    fence(Ordering::SeqCst);
    unsafe {
        ptr::copy_nonoverlapping(
            XV6_DISK_SHARED_BUFFER_VADDR as *const u8,
            backup.as_mut_ptr(),
            FS_BLOCK_SIZE,
        );
        let mut i = 0usize;
        while i < FS_BLOCK_SIZE {
            let byte = (i as u8).wrapping_mul(31).wrapping_add(0xa5);
            ptr::write((XV6_DISK_SHARED_BUFFER_VADDR as *mut u8).add(i), byte);
            i += 1;
        }
    }
    fence(Ordering::SeqCst);
    let wrote_pattern = write_disk_block(blockno);
    let verified = wrote_pattern && read_disk_block(blockno) && verify_write_pattern();
    unsafe {
        ptr::copy_nonoverlapping(
            backup.as_ptr(),
            XV6_DISK_SHARED_BUFFER_VADDR as *mut u8,
            FS_BLOCK_SIZE,
        );
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
    unsafe { core::slice::from_raw_parts(XV6_DISK_SHARED_BUFFER_VADDR as *const u8, FS_BLOCK_SIZE) }
}

pub(crate) fn shared_block_mut() -> &'static mut [u8] {
    unsafe {
        core::slice::from_raw_parts_mut(XV6_DISK_SHARED_BUFFER_VADDR as *mut u8, FS_BLOCK_SIZE)
    }
}
