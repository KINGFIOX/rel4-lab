use core::sync::atomic::{AtomicU64, Ordering};

use crate::allocator::Allocator;
use crate::arch::current as arch;
use crate::consts::*;
use crate::exec_syscalls::sys_exec;
use crate::fs_syscalls::{
    sys_chdir, sys_close, sys_dup, sys_fstat, sys_link, sys_mkdir, sys_mknod, sys_open, sys_pipe,
    sys_unlink,
};
use crate::io_syscalls::{self, sys_pause, sys_read, sys_write};
use crate::memory_syscalls::{handle_lazy_page_fault, sys_sbrk};
use crate::process_syscalls::{fault_kill, sys_exit, sys_fork, sys_kill, sys_wait};
use crate::types::{SyscallResult, TaskStruct};
use crate::util::warn;

pub(crate) use crate::io_syscalls::pump_vfs_waiters;
pub(crate) use crate::vfs::{
    complete_vfs_async_reply, has_active_vfs_async_requests, init_vfs_client, init_vfs_process,
    use_deferred_reply_slot,
};

pub(crate) fn should_defer_vfs_syscall(_child: &crate::types::TaskStruct, mrs: &[u64; 64]) -> bool {
    matches!(
        Xv6Syscall::from_raw(arch::syscall_number(mrs)),
        Some(
            Xv6Syscall::Fork
                | Xv6Syscall::Exit
                | Xv6Syscall::Kill
                | Xv6Syscall::Read
                | Xv6Syscall::Write
                | Xv6Syscall::Open
                | Xv6Syscall::Close
                | Xv6Syscall::Dup
                | Xv6Syscall::Fstat
                | Xv6Syscall::Chdir
                | Xv6Syscall::Pipe
                | Xv6Syscall::Link
                | Xv6Syscall::Unlink
                | Xv6Syscall::Mkdir
                | Xv6Syscall::Mknod
                | Xv6Syscall::Exec
        )
    )
}

static TICKS: AtomicU64 = AtomicU64::new(0);

pub(crate) fn tick() {
    TICKS.fetch_add(1, Ordering::Relaxed);
}

pub(crate) fn pump_sleep_waiters(alloc: &mut Allocator, procs: &mut [TaskStruct; MAX_PROCS]) {
    io_syscalls::pump_sleep_waiters(alloc, procs, ticks_now());
}

pub(crate) fn handle_xv6_syscall(
    alloc: &mut Allocator,
    procs: &mut [TaskStruct; MAX_PROCS],
    proc_idx: usize,
    mrs: &[u64; 64],
) -> SyscallResult {
    let sysno = Xv6Syscall::from_raw(arch::syscall_number(mrs));
    let a0 = arch::syscall_arg(mrs, 0);
    let a1 = arch::syscall_arg(mrs, 1);
    let a2 = arch::syscall_arg(mrs, 2);

    let ret = match sysno {
        Some(Xv6Syscall::Fork) => {
            return SyscallResult::Reply(sys_fork(alloc, procs, proc_idx, mrs));
        }
        Some(Xv6Syscall::Exit) => {
            return sys_exit(alloc, procs, proc_idx, a0 as i32);
        }
        Some(Xv6Syscall::Wait) => return sys_wait(alloc, procs, proc_idx, a0, mrs),
        Some(Xv6Syscall::Write) => {
            let result = sys_write(
                alloc,
                &mut procs[proc_idx],
                a0 as usize,
                a1,
                a2 as usize,
                mrs,
            );
            pump_vfs_waiters(alloc, procs);
            return result;
        }
        Some(Xv6Syscall::Read) => {
            let result = sys_read(
                alloc,
                &mut procs[proc_idx],
                a0 as usize,
                a1,
                a2 as usize,
                mrs,
            );
            pump_vfs_waiters(alloc, procs);
            return result;
        }
        Some(Xv6Syscall::Open) => return sys_open(alloc, &mut procs[proc_idx], a0, a1 as u32, mrs),
        Some(Xv6Syscall::Close) => return sys_close(alloc, &mut procs[proc_idx], a0 as usize, mrs),
        Some(Xv6Syscall::Dup) => return sys_dup(alloc, &mut procs[proc_idx], a0 as usize, mrs),
        Some(Xv6Syscall::Fstat) => {
            return sys_fstat(alloc, &mut procs[proc_idx], a0 as usize, a1, mrs);
        }
        Some(Xv6Syscall::Sbrk) => sys_sbrk(alloc, &mut procs[proc_idx], a0 as i64, a1),
        Some(Xv6Syscall::GetPid) => procs[proc_idx].pid as i64,
        Some(Xv6Syscall::Uptime) => ticks_now() as i64,
        Some(Xv6Syscall::Pause) => {
            return sys_pause(alloc, &mut procs[proc_idx], a0 as i64, ticks_now(), mrs);
        }
        Some(Xv6Syscall::Kill) => sys_kill(alloc, procs, a0 as i64),
        Some(Xv6Syscall::Chdir) => return sys_chdir(alloc, &mut procs[proc_idx], a0, mrs),
        Some(Xv6Syscall::Pipe) => return sys_pipe(alloc, &mut procs[proc_idx], a0, mrs),
        Some(Xv6Syscall::Mknod) => {
            return sys_mknod(alloc, &mut procs[proc_idx], a0, a1 as u16, a2 as u16, mrs);
        }
        Some(Xv6Syscall::Exec) => return sys_exec(alloc, &mut procs[proc_idx], a0, a1),
        Some(Xv6Syscall::Unlink) => return sys_unlink(alloc, &mut procs[proc_idx], a0, mrs),
        Some(Xv6Syscall::Link) => return sys_link(alloc, &mut procs[proc_idx], a0, a1, mrs),
        Some(Xv6Syscall::Mkdir) => return sys_mkdir(alloc, &mut procs[proc_idx], a0, mrs),
        _ => -1,
    };
    SyscallResult::Reply(ret)
}

pub(crate) fn handle_xv6_fault(
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
            "xv6-host: unhandled VM fault pid={} addr={:#x} fsr={} heap_start={:#x} brk={:#x} limit={:#x}",
            procs[proc_idx].pid,
            fault_addr,
            fsr,
            procs[proc_idx].heap_start,
            procs[proc_idx].brk,
            CHILD_HEAP_LIMIT
        );
    }
    fault_kill(alloc, procs, proc_idx, label)
}

fn ticks_now() -> u64 {
    TICKS.load(Ordering::Relaxed)
}
