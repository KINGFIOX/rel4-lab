use core::sync::atomic::{AtomicU64, Ordering};

use crate::allocator::Allocator;
use crate::arch::current as arch;
use crate::consts::*;
use crate::exec_syscalls::sys_execve;
use crate::fs_syscalls::{
    sys_chdir, sys_close, sys_dup, sys_dup3, sys_fstat, sys_getcwd, sys_lseek, sys_mkdirat,
    sys_openat, sys_pipe2, sys_unlinkat,
};
use crate::io_syscalls::{
    self, sys_clock_getres, sys_clock_gettime, sys_gettimeofday, sys_nanosleep, sys_read,
    sys_write, sys_writev,
};
use crate::memory_syscalls::{handle_lazy_page_fault, sys_brk, sys_mmap, sys_mprotect, sys_munmap};
use crate::process_syscalls::{
    fault_kill, sys_clone, sys_exit, sys_getpid, sys_getppid, sys_gettid, sys_kill,
    sys_set_tid_address, sys_uname, sys_wait4, sys_waitid,
};
use crate::types::{SyscallResult, TaskStruct};

pub(crate) use crate::io_syscalls::pump_vfs_waiters;
pub(crate) use crate::vfs::{
    complete_vfs_async_reply, has_active_vfs_async_requests, init_vfs_client, init_vfs_process,
    use_deferred_reply_slot,
};

pub(crate) fn should_defer_vfs_syscall(mrs: &[u64; 64]) -> bool {
    matches!(
        arch::syscall_number(mrs),
        SYS_CLONE
            | SYS_EXIT
            | SYS_EXIT_GROUP
            | SYS_KILL
            | SYS_READ
            | SYS_WRITE
            | SYS_READV
            | SYS_WRITEV
            | SYS_OPENAT
            | SYS_CLOSE
            | SYS_DUP
            | SYS_DUP3
            | SYS_FSTAT
            | SYS_CHDIR
            | SYS_PIPE2
            | SYS_UNLINKAT
            | SYS_MKDIRAT
            | SYS_EXECVE
            | SYS_GETCWD
            | SYS_LSEEK
    )
}

static TICKS: AtomicU64 = AtomicU64::new(0);

pub(crate) fn tick() {
    TICKS.fetch_add(1, Ordering::Relaxed);
}

pub(crate) fn ticks_now() -> u64 {
    TICKS.load(Ordering::Relaxed)
}

pub(crate) fn pump_sleep_waiters(procs: &mut [TaskStruct; MAX_PROCS]) {
    io_syscalls::pump_sleep_waiters(procs, ticks_now());
}

pub(crate) fn handle_linux_syscall(
    alloc: &mut Allocator,
    procs: &mut [TaskStruct; MAX_PROCS],
    proc_idx: usize,
    mrs: &[u64; 64],
) -> SyscallResult {
    let sysno = arch::syscall_number(mrs);
    let a0 = arch::syscall_arg(mrs, 0);
    let a1 = arch::syscall_arg(mrs, 1);
    let a2 = arch::syscall_arg(mrs, 2);
    let a3 = arch::syscall_arg(mrs, 3);
    let a4 = arch::syscall_arg(mrs, 4);
    let a5 = arch::syscall_arg(mrs, 5);

    match sysno {
        SYS_GETCWD => sys_getcwd(alloc, &mut procs[proc_idx], a0, a1, mrs),
        SYS_DUP => sys_dup(alloc, &mut procs[proc_idx], a0 as usize, mrs),
        SYS_DUP3 => sys_dup3(
            alloc,
            &mut procs[proc_idx],
            a0 as usize,
            a1 as usize,
            a2 as u32,
            mrs,
        ),
        SYS_IOCTL => sys_ioctl(&procs[proc_idx], a0 as usize, a1),
        SYS_MKDIRAT => sys_mkdirat(alloc, &mut procs[proc_idx], a0 as i32, a1, a2 as u32, mrs),
        SYS_UNLINKAT => sys_unlinkat(alloc, &mut procs[proc_idx], a0 as i32, a1, a2 as u32, mrs),
        SYS_CHDIR => sys_chdir(alloc, &mut procs[proc_idx], a0, mrs),
        SYS_OPENAT => sys_openat(
            alloc,
            &mut procs[proc_idx],
            a0 as i32,
            a1,
            a2 as u32,
            a3 as u32,
            mrs,
        ),
        SYS_CLOSE => sys_close(alloc, &mut procs[proc_idx], a0 as usize, mrs),
        SYS_PIPE2 => sys_pipe2(alloc, &mut procs[proc_idx], a0, a1 as u32, mrs),
        SYS_LSEEK => sys_lseek(alloc, &mut procs[proc_idx], a0 as usize, a1 as i64, a2, mrs),
        SYS_READ => {
            let result = sys_read(
                alloc,
                &mut procs[proc_idx],
                a0 as usize,
                a1,
                a2 as usize,
                mrs,
            );
            pump_vfs_waiters(alloc, procs);
            result
        }
        SYS_WRITE => {
            let result = sys_write(
                alloc,
                &mut procs[proc_idx],
                a0 as usize,
                a1,
                a2 as usize,
                mrs,
            );
            pump_vfs_waiters(alloc, procs);
            result
        }
        SYS_WRITEV => sys_writev(
            alloc,
            &mut procs[proc_idx],
            a0 as usize,
            a1,
            a2 as usize,
            mrs,
        ),
        SYS_READV => sys_read(
            alloc,
            &mut procs[proc_idx],
            a0 as usize,
            a1,
            a2 as usize,
            mrs,
        ),
        SYS_FSTAT => sys_fstat(alloc, &mut procs[proc_idx], a0 as usize, a1, mrs),
        SYS_EXIT | SYS_EXIT_GROUP => sys_exit(alloc, procs, proc_idx, a0 as i32),
        SYS_WAITID => sys_waitid(
            alloc, procs, proc_idx, a0 as u32, a1 as i64, a2, a3 as u32, mrs,
        ),
        SYS_SET_TID_ADDRESS => SyscallResult::Reply(sys_set_tid_address(&mut procs[proc_idx], a0)),
        SYS_SET_ROBUST_LIST | SYS_GET_ROBUST_LIST => SyscallResult::Reply(0),
        SYS_NANOSLEEP => sys_nanosleep(a0, a1),
        #[cfg(target_arch = "x86_64")]
        SYS_PAUSE => {
            unsafe {
                sel4_user::sel4_yield();
            }
            SyscallResult::Reply(0)
        }
        SYS_CLOCK_GETTIME => sys_clock_gettime(alloc, &procs[proc_idx], a0, a1),
        SYS_CLOCK_GETRES => sys_clock_getres(alloc, &procs[proc_idx], a0, a1),
        SYS_CLOCK_NANOSLEEP => sys_nanosleep(a2, a3),
        SYS_SCHED_YIELD => {
            unsafe {
                sel4_user::sel4_yield();
            }
            SyscallResult::Reply(0)
        }
        SYS_KILL => SyscallResult::Reply(sys_kill(alloc, procs, a0 as i64, a1)),
        SYS_RT_SIGACTION | SYS_RT_SIGPROCMASK | SYS_RT_SIGRETURN => SyscallResult::Reply(0),
        SYS_UNAME => sys_uname(alloc, &procs[proc_idx], a0),
        SYS_PRCTL => SyscallResult::Reply(0),
        SYS_GETTIMEOFDAY => sys_gettimeofday(alloc, &procs[proc_idx], a0, a1),
        SYS_GETPID => SyscallResult::Reply(sys_getpid(&procs[proc_idx])),
        SYS_GETPPID => SyscallResult::Reply(sys_getppid(&procs[proc_idx])),
        SYS_GETUID | SYS_GETEUID | SYS_GETGID | SYS_GETEGID => SyscallResult::Reply(0),
        SYS_GETTID => SyscallResult::Reply(sys_gettid(&procs[proc_idx])),
        SYS_BRK => SyscallResult::Reply(sys_brk(alloc, &mut procs[proc_idx], a0)),
        SYS_MUNMAP => SyscallResult::Reply(sys_munmap(alloc, &mut procs[proc_idx], a0, a1)),
        SYS_CLONE => sys_clone(alloc, procs, proc_idx, a0, a1, a2, a3, a4, mrs),
        SYS_EXECVE => sys_execve(alloc, &mut procs[proc_idx], a0, a1, a2),
        SYS_MMAP => SyscallResult::Reply(sys_mmap(
            alloc,
            &mut procs[proc_idx],
            a0,
            a1,
            a2 as u32,
            a3 as u32,
            a4 as i32,
            a5,
        )),
        SYS_MPROTECT => SyscallResult::Reply(sys_mprotect(&procs[proc_idx], a0, a1, a2 as u32)),
        SYS_WAIT4 => sys_wait4(alloc, procs, proc_idx, a0 as i64, a1, a2 as u32, a3, mrs),
        SYS_GETRANDOM => sys_getrandom(alloc, &procs[proc_idx], a0, a1 as usize, a2),
        SYS_UMASK => SyscallResult::Reply(0o022),
        SYS_FCNTL | SYS_GETDENTS64 | SYS_NEWFSTATAT | SYS_STATX | SYS_LINKAT | SYS_MKNODAT
        | SYS_FACCESSAT | SYS_READLINKAT | SYS_PPOLL | SYS_FUTEX | SYS_CLONE3 | SYS_SOCKET
        | SYS_PTRACE | SYS_MREMAP | SYS_PRLIMIT64 | SYS_SYSINFO | SYS_GETRUSAGE | SYS_TKILL
        | SYS_TGKILL => SyscallResult::err(ENOSYS),
        _ => SyscallResult::err(ENOSYS),
    }
}

pub(crate) fn handle_linux_fault(
    alloc: &mut Allocator,
    procs: &mut [TaskStruct; MAX_PROCS],
    proc_idx: usize,
    label: u64,
    mrs: &[u64; 64],
) -> SyscallResult {
    if label == FAULT_VM_FAULT {
        let fault_addr = arch::vm_fault_addr(mrs);
        let fsr = arch::vm_fault_status(mrs);
        if handle_lazy_page_fault(alloc, &mut procs[proc_idx], fault_addr, fsr) {
            return SyscallResult::ReplyFrame([0; arch::FAULT_REPLY_WORDS]);
        }
        warn!(
            "linux-compat: unhandled VM fault pid={} addr={:#x} fsr={} heap_start={:#x} brk={:#x}",
            procs[proc_idx].pid, fault_addr, fsr, procs[proc_idx].heap_start, procs[proc_idx].brk
        );
    }
    fault_kill(alloc, procs, proc_idx, label)
}

fn sys_ioctl(child: &TaskStruct, fd: usize, _cmd: u64) -> SyscallResult {
    if crate::vfs::fd_file(child, fd).is_none() {
        return SyscallResult::err(EBADF);
    }
    if fd <= 2 {
        return SyscallResult::err(ENOTTY);
    }
    SyscallResult::err(ENOTTY)
}

fn sys_getrandom(
    alloc: &mut Allocator,
    child: &TaskStruct,
    buf: u64,
    len: usize,
    _flags: u64,
) -> SyscallResult {
    if len == 0 {
        return SyscallResult::Reply(0);
    }
    if buf == 0 {
        return SyscallResult::err(EFAULT);
    }
    let n = core::cmp::min(len, 256);
    let mut bytes = [0u8; 256];
    let mut seed = ticks_now()
        .wrapping_mul(0x9e37_79b9)
        .wrapping_add(child.pid);
    let mut i = 0usize;
    while i < n {
        seed = seed.wrapping_mul(1664525).wrapping_add(1013904223);
        bytes[i] = (seed >> 16) as u8;
        i += 1;
    }
    if !crate::child::copy_to_child(alloc, child, buf, &bytes[..n]) {
        return SyscallResult::err(EFAULT);
    }
    SyscallResult::Reply(n as i64)
}

use crate::util::warn;
