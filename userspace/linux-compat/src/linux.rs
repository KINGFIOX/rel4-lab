use core::sync::atomic::{AtomicU64, Ordering};

use crate::arch::current as arch;
use crate::consts::*;
use crate::exec_syscalls::sys_execve;
use crate::fs_syscalls::{
    sys_chdir, sys_close, sys_dup, sys_dup3, sys_fstat, sys_getcwd, sys_lseek, sys_mkdirat,
    sys_openat, sys_pipe2, sys_unlinkat,
};
use crate::host::{find_proc, with_host};
use crate::io_syscalls::{
    sys_clock_getres, sys_clock_gettime, sys_gettimeofday, sys_nanosleep, sys_read, sys_write,
    sys_writev,
};
use crate::memory_syscalls::{handle_lazy_page_fault, sys_brk, sys_mmap, sys_mprotect, sys_munmap};
use crate::process_syscalls::{
    fault_kill, sys_clone, sys_exit, sys_getpid, sys_getppid, sys_gettid, sys_kill,
    sys_set_tid_address, sys_uname, sys_wait4, sys_waitid,
};
use crate::types::SyscallResult;
use crate::vfs::{acquire_vfs, fd_file};

pub(crate) use crate::vfs::{complete_vfs_async_reply, init_vfs_client, init_vfs_process};

static TICKS: AtomicU64 = AtomicU64::new(0);

pub(crate) fn tick() {
    TICKS.fetch_add(1, Ordering::Relaxed);
}

pub(crate) fn ticks_now() -> u64 {
    TICKS.load(Ordering::Relaxed)
}

pub(crate) async fn handle_linux_syscall(
    pid: u64,
    mrs: &[u64; 64],
    reply_slot: u64,
) -> SyscallResult {
    let sysno = arch::syscall_number(mrs);
    let a0 = arch::syscall_arg(mrs, 0);
    let a1 = arch::syscall_arg(mrs, 1);
    let a2 = arch::syscall_arg(mrs, 2);
    let a3 = arch::syscall_arg(mrs, 3);
    let a4 = arch::syscall_arg(mrs, 4);
    let a5 = arch::syscall_arg(mrs, 5);

    match sysno {
        SYS_GETCWD => sys_getcwd(pid, a0, a1),
        SYS_DUP => sys_dup(pid, a0 as usize).await,
        SYS_DUP3 => sys_dup3(pid, a0 as usize, a1 as usize, a2 as u32).await,
        SYS_IOCTL => with_host(|_, procs| {
            let Some(idx) = find_proc(procs, pid) else {
                return SyscallResult::err(ESRCH);
            };
            sys_ioctl(&procs[idx], a0 as usize)
        }),
        SYS_MKDIRAT => sys_mkdirat(pid, a0 as i32, a1, a2 as u32).await,
        SYS_UNLINKAT => sys_unlinkat(pid, a0 as i32, a1, a2 as u32).await,
        SYS_CHDIR => sys_chdir(pid, a0).await,
        SYS_OPENAT => sys_openat(pid, a0 as i32, a1, a2 as u32, a3 as u32).await,
        SYS_CLOSE => sys_close(pid, a0 as usize).await,
        SYS_PIPE2 => sys_pipe2(pid, a0, a1 as u32).await,
        SYS_LSEEK => sys_lseek(pid, a0 as usize, a1 as i64, a2).await,
        SYS_READ | SYS_READV => sys_read(pid, a0 as usize, a1, a2 as usize).await,
        SYS_WRITE => sys_write(pid, a0 as usize, a1, a2 as usize).await,
        SYS_WRITEV => sys_writev(pid, a0 as usize, a1, a2 as usize).await,
        SYS_FSTAT => sys_fstat(pid, a0 as usize, a1).await,
        SYS_EXIT | SYS_EXIT_GROUP => {
            let _permit = acquire_vfs().await;
            with_host(|alloc, procs| {
                let Some(idx) = find_proc(procs, pid) else {
                    return SyscallResult::err(ESRCH);
                };
                sys_exit(alloc, procs, idx, a0 as i32)
            })
        }
        SYS_WAITID => with_host(|alloc, procs| {
            let Some(idx) = find_proc(procs, pid) else {
                return SyscallResult::err(ESRCH);
            };
            sys_waitid(
                alloc, procs, idx, a0 as u32, a1 as i64, a2, a3 as u32, mrs, reply_slot,
            )
        }),
        SYS_SET_TID_ADDRESS => with_host(|_, procs| {
            let Some(idx) = find_proc(procs, pid) else {
                return SyscallResult::err(ESRCH);
            };
            SyscallResult::Reply(sys_set_tid_address(&mut procs[idx], a0))
        }),
        SYS_SET_ROBUST_LIST | SYS_GET_ROBUST_LIST => SyscallResult::Reply(0),
        SYS_NANOSLEEP => sys_nanosleep(pid, a0, a1).await,
        #[cfg(target_arch = "x86_64")]
        SYS_PAUSE => {
            unsafe {
                sel4_user::sel4_yield();
            }
            SyscallResult::Reply(0)
        }
        SYS_CLOCK_GETTIME => sys_clock_gettime(pid, a0, a1),
        SYS_CLOCK_GETRES => sys_clock_getres(pid, a0, a1),
        SYS_CLOCK_NANOSLEEP => sys_nanosleep(pid, a2, a3).await,
        SYS_SCHED_YIELD => {
            unsafe {
                sel4_user::sel4_yield();
            }
            SyscallResult::Reply(0)
        }
        SYS_KILL => {
            let _permit = acquire_vfs().await;
            with_host(|alloc, procs| SyscallResult::Reply(sys_kill(alloc, procs, a0 as i64, a1)))
        }
        SYS_RT_SIGACTION | SYS_RT_SIGPROCMASK | SYS_RT_SIGRETURN => SyscallResult::Reply(0),
        SYS_UNAME => with_host(|alloc, procs| {
            let Some(idx) = find_proc(procs, pid) else {
                return SyscallResult::err(ESRCH);
            };
            sys_uname(alloc, &procs[idx], a0)
        }),
        SYS_PRCTL => SyscallResult::Reply(0),
        SYS_GETTIMEOFDAY => sys_gettimeofday(pid, a0, a1),
        SYS_GETPID => with_host(|_, procs| {
            let Some(idx) = find_proc(procs, pid) else {
                return SyscallResult::err(ESRCH);
            };
            SyscallResult::Reply(sys_getpid(&procs[idx]))
        }),
        SYS_GETPPID => with_host(|_, procs| {
            let Some(idx) = find_proc(procs, pid) else {
                return SyscallResult::err(ESRCH);
            };
            SyscallResult::Reply(sys_getppid(&procs[idx]))
        }),
        SYS_GETUID | SYS_GETEUID | SYS_GETGID | SYS_GETEGID => SyscallResult::Reply(0),
        SYS_GETTID => with_host(|_, procs| {
            let Some(idx) = find_proc(procs, pid) else {
                return SyscallResult::err(ESRCH);
            };
            SyscallResult::Reply(sys_gettid(&procs[idx]))
        }),
        SYS_BRK => with_host(|alloc, procs| {
            let Some(idx) = find_proc(procs, pid) else {
                return SyscallResult::err(ESRCH);
            };
            SyscallResult::Reply(sys_brk(alloc, &mut procs[idx], a0))
        }),
        SYS_MUNMAP => with_host(|alloc, procs| {
            let Some(idx) = find_proc(procs, pid) else {
                return SyscallResult::err(ESRCH);
            };
            SyscallResult::Reply(sys_munmap(alloc, &mut procs[idx], a0, a1))
        }),
        SYS_CLONE => {
            let _permit = acquire_vfs().await;
            with_host(|alloc, procs| {
                let Some(idx) = find_proc(procs, pid) else {
                    return SyscallResult::err(ESRCH);
                };
                sys_clone(alloc, procs, idx, a0, a1, a2, a3, a4, mrs)
            })
        }
        SYS_EXECVE => {
            let _permit = acquire_vfs().await;
            with_host(|alloc, procs| {
                let Some(idx) = find_proc(procs, pid) else {
                    return SyscallResult::err(ESRCH);
                };
                sys_execve(alloc, &mut procs[idx], a0, a1, a2)
            })
        }
        SYS_MMAP => with_host(|alloc, procs| {
            let Some(idx) = find_proc(procs, pid) else {
                return SyscallResult::err(ESRCH);
            };
            SyscallResult::Reply(sys_mmap(
                alloc,
                &mut procs[idx],
                a0,
                a1,
                a2 as u32,
                a3 as u32,
                a4 as i32,
                a5,
            ))
        }),
        SYS_MPROTECT => with_host(|_, procs| {
            let Some(idx) = find_proc(procs, pid) else {
                return SyscallResult::err(ESRCH);
            };
            SyscallResult::Reply(sys_mprotect(&procs[idx], a0, a1, a2 as u32))
        }),
        SYS_WAIT4 => with_host(|alloc, procs| {
            let Some(idx) = find_proc(procs, pid) else {
                return SyscallResult::err(ESRCH);
            };
            sys_wait4(
                alloc, procs, idx, a0 as i64, a1, a2 as u32, a3, mrs, reply_slot,
            )
        }),
        SYS_GETRANDOM => with_host(|alloc, procs| {
            let Some(idx) = find_proc(procs, pid) else {
                return SyscallResult::err(ESRCH);
            };
            sys_getrandom(alloc, &procs[idx], a0, a1 as usize, a2)
        }),
        SYS_UMASK => SyscallResult::Reply(0o022),
        SYS_FCNTL | SYS_GETDENTS64 | SYS_NEWFSTATAT | SYS_STATX | SYS_LINKAT | SYS_MKNODAT
        | SYS_FACCESSAT | SYS_READLINKAT | SYS_PPOLL | SYS_FUTEX | SYS_CLONE3 | SYS_SOCKET
        | SYS_PTRACE | SYS_MREMAP | SYS_PRLIMIT64 | SYS_SYSINFO | SYS_GETRUSAGE | SYS_TKILL
        | SYS_TGKILL => SyscallResult::err(ENOSYS),
        _ => SyscallResult::err(ENOSYS),
    }
}

pub(crate) async fn handle_linux_fault(pid: u64, label: u64, mrs: &[u64; 64]) -> SyscallResult {
    let handled = with_host(|alloc, procs| {
        let Some(proc_idx) = find_proc(procs, pid) else {
            return Some(SyscallResult::err(ESRCH));
        };
        if label == FAULT_VM_FAULT {
            let fault_addr = arch::vm_fault_addr(mrs);
            let fsr = arch::vm_fault_status(mrs);
            if handle_lazy_page_fault(alloc, &mut procs[proc_idx], fault_addr, fsr) {
                return Some(SyscallResult::ReplyFrame([0; arch::FAULT_REPLY_WORDS]));
            }
            warn!(
                "linux-compat: unhandled VM fault pid={} addr={:#x} fsr={} heap_start={:#x} brk={:#x}",
                procs[proc_idx].pid,
                fault_addr,
                fsr,
                procs[proc_idx].heap_start,
                procs[proc_idx].brk
            );
        }
        None
    });
    if let Some(result) = handled {
        return result;
    }
    let _permit = acquire_vfs().await;
    with_host(|alloc, procs| {
        let Some(proc_idx) = find_proc(procs, pid) else {
            return SyscallResult::err(ESRCH);
        };
        fault_kill(alloc, procs, proc_idx, label)
    })
}

fn sys_ioctl(child: &crate::types::TaskStruct, fd: usize) -> SyscallResult {
    if fd_file(child, fd).is_none() {
        return SyscallResult::err(EBADF);
    }
    if fd <= 2 {
        return SyscallResult::err(ENOTTY);
    }
    SyscallResult::err(ENOTTY)
}

fn sys_getrandom(
    alloc: &mut crate::allocator::Allocator,
    child: &crate::types::TaskStruct,
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
