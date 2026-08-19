use crate::allocator::Allocator;
use crate::arch::current as arch;
use crate::child::{copy_from_child, copy_to_child};
use crate::consts::*;
use crate::reply_caps;
use crate::types::{SyscallResult, TaskStruct};
use crate::vfs::{resume_vfs_waiter_async, start_vfs_read_request, start_vfs_write_request};
use linux_abi::{LinuxTimespec, LinuxTimeval};
use sel4_user::msg_info;
use sel4_user::sel4_yield;

pub(crate) fn sys_write(
    alloc: &mut Allocator,
    child: &mut TaskStruct,
    fd: usize,
    buf: u64,
    len: usize,
    mrs: &[u64; 64],
) -> SyscallResult {
    if crate::vfs::fd_file(child, fd).is_none() {
        return SyscallResult::err(EBADF);
    }
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
    if crate::vfs::fd_file(child, fd).is_none() {
        return SyscallResult::err(EBADF);
    }
    if len == 0 {
        return SyscallResult::Reply(0);
    }
    start_vfs_read_request(alloc, child, mrs, fd, dst, len)
}

pub(crate) fn sys_writev(
    alloc: &mut Allocator,
    child: &mut TaskStruct,
    fd: usize,
    iov_ptr: u64,
    iovcnt: usize,
    mrs: &[u64; 64],
) -> SyscallResult {
    if iovcnt == 0 {
        return SyscallResult::Reply(0);
    }
    if iovcnt > 8 {
        return SyscallResult::err(EINVAL);
    }
    let mut iov = [0u8; 16];
    if !copy_from_child(alloc, child, iov_ptr, &mut iov) {
        return SyscallResult::err(EFAULT);
    }
    let base = crate::util::read_u64(&iov, 0);
    let len = crate::util::read_u64(&iov, 8) as usize;
    sys_write(alloc, child, fd, base, len, mrs)
}

pub(crate) fn sys_nanosleep(req_ptr: u64, _rem_ptr: u64) -> SyscallResult {
    if req_ptr == 0 {
        return SyscallResult::err(EFAULT);
    }
    unsafe {
        sel4_yield();
    }
    SyscallResult::Reply(0)
}

pub(crate) fn sys_clock_gettime(
    alloc: &mut Allocator,
    child: &TaskStruct,
    clock_id: u64,
    ts_ptr: u64,
) -> SyscallResult {
    if ts_ptr == 0 {
        return SyscallResult::err(EFAULT);
    }
    if clock_id != CLOCK_REALTIME && clock_id != CLOCK_MONOTONIC {
        return SyscallResult::err(EINVAL);
    }
    let ts = ticks_to_timespec(crate::linux::ticks_now());
    if !copy_timespec(alloc, child, ts_ptr, &ts) {
        return SyscallResult::err(EFAULT);
    }
    SyscallResult::Reply(0)
}

pub(crate) fn sys_clock_getres(
    alloc: &mut Allocator,
    child: &TaskStruct,
    clock_id: u64,
    ts_ptr: u64,
) -> SyscallResult {
    if clock_id != CLOCK_REALTIME && clock_id != CLOCK_MONOTONIC {
        return SyscallResult::err(EINVAL);
    }
    if ts_ptr == 0 {
        return SyscallResult::Reply(0);
    }
    let ts = LinuxTimespec {
        tv_sec: 0,
        tv_nsec: 10_000_000,
    };
    if !copy_timespec(alloc, child, ts_ptr, &ts) {
        return SyscallResult::err(EFAULT);
    }
    SyscallResult::Reply(0)
}

pub(crate) fn sys_gettimeofday(
    alloc: &mut Allocator,
    child: &TaskStruct,
    tv_ptr: u64,
    _tz_ptr: u64,
) -> SyscallResult {
    if tv_ptr == 0 {
        return SyscallResult::Reply(0);
    }
    let ts = ticks_to_timespec(crate::linux::ticks_now());
    let tv = LinuxTimeval {
        tv_sec: ts.tv_sec,
        tv_usec: ts.tv_nsec / 1000,
    };
    let bytes = unsafe {
        core::slice::from_raw_parts(
            core::ptr::addr_of!(tv) as *const u8,
            core::mem::size_of::<LinuxTimeval>(),
        )
    };
    if !copy_to_child(alloc, child, tv_ptr, bytes) {
        return SyscallResult::err(EFAULT);
    }
    SyscallResult::Reply(0)
}

fn ticks_to_timespec(ticks: u64) -> LinuxTimespec {
    LinuxTimespec {
        tv_sec: (ticks / 100) as i64,
        tv_nsec: ((ticks % 100) * 10_000_000) as i64,
    }
}

fn copy_timespec(alloc: &mut Allocator, child: &TaskStruct, dest: u64, ts: &LinuxTimespec) -> bool {
    let bytes = unsafe {
        core::slice::from_raw_parts(
            core::ptr::addr_of!(*ts) as *const u8,
            core::mem::size_of::<LinuxTimespec>(),
        )
    };
    copy_to_child(alloc, child, dest, bytes)
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

pub(crate) fn pump_sleep_waiters(procs: &mut [TaskStruct; MAX_PROCS], now: u64) {
    for child in procs.iter_mut() {
        if child.state != PROC_SLEEPING || child.sleep_reply_slot == 0 {
            continue;
        }
        if now.wrapping_sub(child.sleep_deadline) >= (1u64 << 63) {
            continue;
        }
        let mut reply_mrs = child.sleep_reply_mrs;
        arch::set_syscall_return_value(&mut reply_mrs, 0);
        reply_caps::send_and_release(
            child.sleep_reply_slot,
            msg_info(0, 0, 0, arch::FAULT_REPLY_WORDS as u64),
            &reply_mrs,
        );
        child.state = PROC_RUNNABLE;
        clear_sleep_block(child);
    }
}

pub(crate) fn drop_blocked_reply_caps(child: &mut TaskStruct) {
    if child.wait_reply_slot != 0 {
        reply_caps::stop_and_release(child.wait_reply_slot);
        clear_wait_block(child);
    }
    if child.vfs_reply_slot != 0 {
        reply_caps::stop_and_release(child.vfs_reply_slot);
        clear_vfs_block(child);
    }
    if child.sleep_reply_slot != 0 {
        reply_caps::stop_and_release(child.sleep_reply_slot);
        clear_sleep_block(child);
    }
}

pub(crate) fn save_blocked_reply(mrs: &[u64; 64]) -> (u64, arch::FaultReplyFrame) {
    let reply_slot = reply_caps::take_current();
    let reply_mrs = arch::syscall_reply_frame(mrs);
    (reply_slot, reply_mrs)
}

pub(crate) fn clear_wait_block(child: &mut TaskStruct) {
    child.wait_status_ptr = 0;
    child.wait_pid = -1;
    child.wait_options = 0;
    child.wait_reply_slot = 0;
    child.wait_reply_mrs = [0; arch::FAULT_REPLY_WORDS];
}

fn pump_vfs_readers(alloc: &mut Allocator, procs: &mut [TaskStruct; MAX_PROCS]) -> bool {
    for child in procs.iter_mut() {
        if child.state != PROC_VFS_READ {
            continue;
        }
        if child.vfs_done >= child.vfs_len {
            reply_vfs_waiter(child, child.vfs_done as i64);
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
            reply_vfs_waiter(child, child.vfs_done as i64);
            continue;
        }
        if resume_vfs_waiter_async(alloc, child, PROC_VFS_WRITE) {
            return true;
        }
    }
    false
}

fn reply_vfs_waiter(child: &mut TaskStruct, ret: i64) {
    let mut reply_mrs = child.vfs_reply_mrs;
    arch::set_syscall_return_value(&mut reply_mrs, ret as u64);
    reply_caps::send_and_release(
        child.vfs_reply_slot,
        msg_info(0, 0, 0, arch::FAULT_REPLY_WORDS as u64),
        &reply_mrs,
    );
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
