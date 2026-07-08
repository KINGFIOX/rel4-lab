use core::ptr;
use core::sync::atomic::{Ordering, fence};

use sel4_user::{debug, msg_info, sel4_send, warn};
use xv6_abi::{
    DiskRequestOp, XV6_DISK_COMPLETION_NTFN_CPTR, XV6_DISK_COMPLETION_RING_ENTRIES,
    XV6_DISK_COMPLETION_RING_VADDR,
};

use crate::layout::{
    COMPLETION_ENTRIES_OFF, COMPLETION_ENTRY_STRIDE, COMPLETION_READ_IDX_OFF,
    COMPLETION_WRITE_IDX_OFF,
};

pub fn send(completion_id: u64, reply_mrs: [u64; 4]) {
    if !enqueue(completion_id, reply_mrs) {
        return;
    }
    trace_signal(completion_id);
    unsafe {
        sel4_send(
            XV6_DISK_COMPLETION_NTFN_CPTR,
            msg_info(DiskRequestOp::Complete.raw(), 0, 0, 0),
            &[],
        );
    }
    trace_signaled(completion_id);
}

fn enqueue(completion_id: u64, reply_mrs: [u64; 4]) -> bool {
    let read_idx = read64(COMPLETION_READ_IDX_OFF);
    let write_idx = read64(COMPLETION_WRITE_IDX_OFF);
    if write_idx.wrapping_sub(read_idx) >= XV6_DISK_COMPLETION_RING_ENTRIES as u64 {
        warn!(
            "virtio-disk-server: completion ring full id={}",
            completion_id
        );
        return false;
    }

    let slot = write_idx % XV6_DISK_COMPLETION_RING_ENTRIES as u64;
    let entry = COMPLETION_ENTRIES_OFF + slot * COMPLETION_ENTRY_STRIDE;
    write64(entry, reply_mrs[0]);
    write64(entry + 8, reply_mrs[1]);
    write64(entry + 16, reply_mrs[2]);
    write64(entry + 24, completion_id);
    write64(entry + 32, reply_mrs[3]);
    fence(Ordering::SeqCst);
    write64(COMPLETION_WRITE_IDX_OFF, write_idx.wrapping_add(1));
    fence(Ordering::SeqCst);
    true
}

fn trace_signal(completion_id: u64) {
    debug!("virtio-disk-server: signal completion={}", completion_id);
}

fn trace_signaled(completion_id: u64) {
    debug!("virtio-disk-server: signaled completion={}", completion_id);
}

fn read64(offset: u64) -> u64 {
    unsafe { ptr::read_volatile((XV6_DISK_COMPLETION_RING_VADDR + offset) as *const u64) }
}

fn write64(offset: u64, value: u64) {
    unsafe { ptr::write_volatile((XV6_DISK_COMPLETION_RING_VADDR + offset) as *mut u64, value) }
}
