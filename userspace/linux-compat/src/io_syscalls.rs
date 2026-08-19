use crate::allocator::Allocator;
use crate::child::{copy_from_child, copy_to_child};
use crate::consts::*;
use crate::host::{find_proc, with_host};
use crate::reply_caps;
use crate::types::{SyscallResult, TaskStruct};
use crate::vfs::{fd_file, vfs_read_request, vfs_write_request};
use linux_abi::{LinuxTimespec, LinuxTimeval};
use sel4_user::sel4_yield;

pub(crate) async fn sys_write(pid: u64, fd: usize, buf: u64, len: usize) -> SyscallResult {
    let Some(()) = with_host(|_, procs| {
        let idx = find_proc(procs, pid)?;
        fd_file(&procs[idx], fd).map(|_| ())
    }) else {
        return SyscallResult::err(EBADF);
    };
    if len == 0 {
        return SyscallResult::Reply(0);
    }
    vfs_write_request(pid, fd, buf, len).await
}

pub(crate) async fn sys_read(pid: u64, fd: usize, dst: u64, len: usize) -> SyscallResult {
    let Some(()) = with_host(|_, procs| {
        let idx = find_proc(procs, pid)?;
        fd_file(&procs[idx], fd).map(|_| ())
    }) else {
        return SyscallResult::err(EBADF);
    };
    if len == 0 {
        return SyscallResult::Reply(0);
    }
    vfs_read_request(pid, fd, dst, len).await
}

pub(crate) async fn sys_writev(pid: u64, fd: usize, iov_ptr: u64, iovcnt: usize) -> SyscallResult {
    if iovcnt == 0 {
        return SyscallResult::Reply(0);
    }
    if iovcnt > 8 {
        return SyscallResult::err(EINVAL);
    }
    let Some((base, len)) = with_host(|alloc, procs| {
        let idx = find_proc(procs, pid)?;
        let mut iov = [0u8; 16];
        if !copy_from_child(alloc, &procs[idx], iov_ptr, &mut iov) {
            return None;
        }
        Some((
            crate::util::read_u64(&iov, 0),
            crate::util::read_u64(&iov, 8) as usize,
        ))
    }) else {
        return SyscallResult::err(EFAULT);
    };
    sys_write(pid, fd, base, len).await
}

pub(crate) async fn sys_nanosleep(pid: u64, req_ptr: u64, _rem_ptr: u64) -> SyscallResult {
    if req_ptr == 0 {
        return SyscallResult::err(EFAULT);
    }
    let Some(ts) = with_host(|alloc, procs| {
        let idx = find_proc(procs, pid)?;
        let mut bytes = [0u8; core::mem::size_of::<LinuxTimespec>()];
        if !copy_from_child(alloc, &procs[idx], req_ptr, &mut bytes) {
            return None;
        }
        Some(LinuxTimespec {
            tv_sec: crate::util::read_u64(&bytes, 0) as i64,
            tv_nsec: crate::util::read_u64(&bytes, 8) as i64,
        })
    }) else {
        return SyscallResult::err(EFAULT);
    };
    if ts.tv_sec < 0 || ts.tv_nsec < 0 || ts.tv_nsec >= 1_000_000_000 {
        return SyscallResult::err(EINVAL);
    }
    let extra_ticks = (ts.tv_sec as u64)
        .saturating_mul(100)
        .saturating_add((ts.tv_nsec as u64) / 10_000_000);
    if extra_ticks == 0 {
        unsafe {
            sel4_yield();
        }
        return SyscallResult::Reply(0);
    }
    sel4_user::rt::sleep_until(sel4_user::rt::now().saturating_add(extra_ticks)).await;
    SyscallResult::Reply(0)
}

pub(crate) fn sys_clock_gettime(pid: u64, clock_id: u64, ts_ptr: u64) -> SyscallResult {
    with_host(|alloc, procs| {
        let Some(idx) = find_proc(procs, pid) else {
            return SyscallResult::err(ESRCH);
        };
        if ts_ptr == 0 {
            return SyscallResult::err(EFAULT);
        }
        if clock_id != CLOCK_REALTIME && clock_id != CLOCK_MONOTONIC {
            return SyscallResult::err(EINVAL);
        }
        let ts = ticks_to_timespec(crate::linux::ticks_now());
        if !copy_timespec(alloc, &procs[idx], ts_ptr, &ts) {
            return SyscallResult::err(EFAULT);
        }
        SyscallResult::Reply(0)
    })
}

pub(crate) fn sys_clock_getres(pid: u64, clock_id: u64, ts_ptr: u64) -> SyscallResult {
    with_host(|alloc, procs| {
        let Some(idx) = find_proc(procs, pid) else {
            return SyscallResult::err(ESRCH);
        };
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
        if !copy_timespec(alloc, &procs[idx], ts_ptr, &ts) {
            return SyscallResult::err(EFAULT);
        }
        SyscallResult::Reply(0)
    })
}

pub(crate) fn sys_gettimeofday(pid: u64, tv_ptr: u64, _tz_ptr: u64) -> SyscallResult {
    with_host(|alloc, procs| {
        let Some(idx) = find_proc(procs, pid) else {
            return SyscallResult::err(ESRCH);
        };
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
        if !copy_to_child(alloc, &procs[idx], tv_ptr, bytes) {
            return SyscallResult::err(EFAULT);
        }
        SyscallResult::Reply(0)
    })
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

pub(crate) fn drop_blocked_reply_caps(child: &mut TaskStruct) {
    if child.wait_reply_slot != 0 {
        reply_caps::stop_and_release(child.wait_reply_slot);
        clear_wait_block(child);
    }
}

pub(crate) fn save_blocked_reply(
    reply_slot: u64,
    mrs: &[u64; 64],
) -> (u64, crate::arch::current::FaultReplyFrame) {
    (reply_slot, crate::arch::current::syscall_reply_frame(mrs))
}

pub(crate) fn clear_wait_block(child: &mut TaskStruct) {
    child.wait_status_ptr = 0;
    child.wait_pid = -1;
    child.wait_options = 0;
    child.wait_reply_slot = 0;
    child.wait_reply_mrs = [0; crate::arch::current::FAULT_REPLY_WORDS];
}
