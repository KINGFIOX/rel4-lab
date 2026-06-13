use crate::allocator::Allocator;
use crate::arch::current as arch;
use crate::consts::*;
use crate::types::{SyscallResult, TaskStruct};
use crate::vfs::{resume_vfs_waiter_async, start_vfs_read_request, start_vfs_write_request};
use sel4_user::{call_checked, msg_info, sel4_send};

pub(crate) fn sys_write(
    alloc: &mut Allocator,
    child: &mut TaskStruct,
    fd: usize,
    buf: u64,
    len: usize,
    mrs: &[u64; 64],
) -> SyscallResult {
    if len == 0 {
        return SyscallResult::Reply(0);
    }
    start_vfs_write_request(alloc, child, mrs, fd, buf, len)
}

pub(crate) fn sys_read(
    alloc: &mut Allocator,
    child: &mut TaskStruct,
    fd: usize,
    dst: u64,
    len: usize,
    mrs: &[u64; 64],
) -> SyscallResult {
    if fd >= MAX_FD || len == 0 {
        return SyscallResult::Reply(if fd < MAX_FD { 0 } else { -1 });
    }
    start_vfs_read_request(alloc, child, mrs, fd, dst, len)
}

pub(crate) fn sys_pause(
    alloc: &mut Allocator,
    child: &mut TaskStruct,
    ticks: i64,
    now: u64,
    mrs: &[u64; 64],
) -> SyscallResult {
    if ticks <= 0 {
        return SyscallResult::Reply(0);
    }
    let (reply_slot, reply_mrs) = save_blocked_reply(alloc, mrs);
    child.state = PROC_SLEEPING;
    child.sleep_deadline = now.wrapping_add(ticks as u64);
    child.sleep_reply_slot = reply_slot;
    child.sleep_reply_mrs = reply_mrs;
    SyscallResult::Block
}

pub(crate) fn pump_vfs_waiters(alloc: &mut Allocator, procs: &mut [TaskStruct; MAX_PROCS]) {
    if crate::vfs::has_active_vfs_async_requests() {
        return;
    }
    if pump_vfs_readers(alloc, procs) {
        return;
    }
    if pump_vfs_writers(alloc, procs) {
        return;
    }
    let _ = pump_vfs_readers(alloc, procs);
}

pub(crate) fn pump_sleep_waiters(
    alloc: &mut Allocator,
    procs: &mut [TaskStruct; MAX_PROCS],
    now: u64,
) {
    for child in procs.iter_mut() {
        if child.state != PROC_SLEEPING || child.sleep_reply_slot == 0 {
            continue;
        }
        if now.wrapping_sub(child.sleep_deadline) >= (1u64 << 63) {
            continue;
        }
        let mut reply_mrs = child.sleep_reply_mrs;
        arch::set_syscall_return_value(&mut reply_mrs, 0);
        unsafe {
            sel4_send(
                child.sleep_reply_slot,
                msg_info(0, 0, 0, arch::FAULT_REPLY_WORDS as u64),
                &reply_mrs,
            );
        }
        alloc.delete_cap_slot(child.sleep_reply_slot);
        child.state = PROC_RUNNABLE;
        clear_sleep_block(child);
    }
}

pub(crate) fn drop_blocked_reply_caps(alloc: &mut Allocator, child: &mut TaskStruct) {
    if child.wait_reply_slot != 0 {
        alloc.delete_cap_slot(child.wait_reply_slot);
        clear_wait_block(child);
    }
    if child.vfs_reply_slot != 0 {
        alloc.delete_cap_slot(child.vfs_reply_slot);
        clear_vfs_block(child);
    }
    if child.sleep_reply_slot != 0 {
        alloc.delete_cap_slot(child.sleep_reply_slot);
        clear_sleep_block(child);
    }
}

pub(crate) fn save_blocked_reply(
    alloc: &mut Allocator,
    mrs: &[u64; 64],
) -> (u64, arch::FaultReplyFrame) {
    let reply_slot = alloc.alloc_slot();
    call_checked(
        ROOT_CNODE,
        LABEL_CNODE_SAVE_CALLER,
        &[],
        &[reply_slot, ROOT_CNODE_DEPTH],
    );
    let reply_mrs = arch::syscall_reply_frame(mrs);
    (reply_slot, reply_mrs)
}

pub(crate) fn clear_wait_block(child: &mut TaskStruct) {
    child.wait_status_ptr = 0;
    child.wait_reply_slot = 0;
    child.wait_reply_mrs = [0; arch::FAULT_REPLY_WORDS];
}

fn pump_vfs_readers(alloc: &mut Allocator, procs: &mut [TaskStruct; MAX_PROCS]) -> bool {
    for child in procs.iter_mut() {
        if child.state != PROC_VFS_READ {
            continue;
        }
        if child.vfs_done >= child.vfs_len {
            reply_vfs_waiter(alloc, child, child.vfs_done as i64);
            continue;
        }
        if resume_vfs_waiter_async(alloc, child, PROC_VFS_READ) {
            return true;
        }
    }
    false
}

fn pump_vfs_writers(alloc: &mut Allocator, procs: &mut [TaskStruct; MAX_PROCS]) -> bool {
    for child in procs.iter_mut() {
        if child.state != PROC_VFS_WRITE {
            continue;
        }
        if child.vfs_done >= child.vfs_len {
            reply_vfs_waiter(alloc, child, child.vfs_done as i64);
            continue;
        }
        if resume_vfs_waiter_async(alloc, child, PROC_VFS_WRITE) {
            return true;
        }
    }
    false
}

fn reply_vfs_waiter(alloc: &mut Allocator, child: &mut TaskStruct, ret: i64) {
    let mut reply_mrs = child.vfs_reply_mrs;
    arch::set_syscall_return_value(&mut reply_mrs, ret as u64);
    unsafe {
        sel4_send(
            child.vfs_reply_slot,
            msg_info(0, 0, 0, arch::FAULT_REPLY_WORDS as u64),
            &reply_mrs,
        );
    }
    alloc.delete_cap_slot(child.vfs_reply_slot);
    child.state = PROC_RUNNABLE;
    clear_vfs_block(child);
}

fn clear_vfs_block(child: &mut TaskStruct) {
    child.vfs_reply_slot = 0;
    child.vfs_reply_mrs = [0; arch::FAULT_REPLY_WORDS];
    child.vfs_fd = 0;
    child.vfs_buf = 0;
    child.vfs_len = 0;
    child.vfs_done = 0;
}

fn clear_sleep_block(child: &mut TaskStruct) {
    child.sleep_deadline = 0;
    child.sleep_reply_slot = 0;
    child.sleep_reply_mrs = [0; arch::FAULT_REPLY_WORDS];
}
