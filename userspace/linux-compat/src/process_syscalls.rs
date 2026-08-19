use crate::allocator::Allocator;
use crate::arch::current as arch;
use crate::child::{
    clear_process_mappings, clone_address_space, clone_page_count, copy_to_child, create_child,
    destroy_child_objects, frame_pool_available, mapping_slots_available, read_user_context,
    write_user_context,
};
use crate::consts::*;
use crate::io_syscalls::{
    clear_wait_block, drop_blocked_reply_caps, pump_vfs_waiters, save_blocked_reply,
};
use crate::memory_syscalls::{
    release_all_sparse_eager, reserve_sparse_eager, sparse_eager_can_clone,
};
use crate::reply_caps;
use crate::types::{SyscallResult, TaskStruct};
use crate::util::{halt_loop, info, warn, write_i32};
use crate::vfs::{close_all_fds, release_cwd_ref, retain_fd_refs, vfs_release_cwd, vfs_retain_cwd};
use core::sync::atomic::{AtomicU64, Ordering};
use linux_abi::{LinuxUtsname, UTS_LEN};
use sel4_user::{call_checked, msg_info};

static NEXT_PID: AtomicU64 = AtomicU64::new(2);
const ARCH_FORK_EXTRA_SLOTS: usize = 0;

pub(crate) fn sys_clone(
    alloc: &mut Allocator,
    procs: &mut [TaskStruct; MAX_PROCS],
    parent_idx: usize,
    flags: u64,
    stack: u64,
    _ptid: u64,
    _tls: u64,
    _ctid: u64,
    mrs: &[u64; 64],
) -> SyscallResult {
    if stack != 0 || (flags & CLONE_VM) != 0 {
        return SyscallResult::err(ENOSYS);
    }
    let pid = sys_fork(alloc, procs, parent_idx, mrs);
    if pid < 0 {
        SyscallResult::Reply(pid)
    } else {
        SyscallResult::Reply(pid)
    }
}

pub(crate) fn sys_fork(
    alloc: &mut Allocator,
    procs: &mut [TaskStruct; MAX_PROCS],
    parent_idx: usize,
    mrs: &[u64; 64],
) -> i64 {
    let Some(slot) = find_free_proc(procs) else {
        return -(EAGAIN as i64);
    };
    let parent = procs[parent_idx];
    let clone_pages = clone_page_count(&parent);
    let frame_slots_needed = clone_pages.saturating_sub(frame_pool_available());
    let child_alias_slots_needed = clone_pages;
    let source_alias_slots_needed = clone_pages;
    let slots_needed = 16usize
        .saturating_add(clone_pages)
        .saturating_add(frame_slots_needed)
        .saturating_add(child_alias_slots_needed)
        .saturating_add(source_alias_slots_needed)
        .saturating_add(ARCH_FORK_EXTRA_SLOTS);
    if alloc.slots_available() < slots_needed.saturating_add(FORK_SLOT_HEADROOM) {
        return -(ENOMEM as i64);
    }
    if mapping_slots_available() < clone_pages.saturating_add(1 + FORK_SLOT_HEADROOM) {
        return -(ENOMEM as i64);
    }
    if !sparse_eager_can_clone(&parent) {
        return -(ENOMEM as i64);
    }

    let pid = NEXT_PID.fetch_add(1, Ordering::Relaxed);

    let mut child = create_child(alloc, slot, pid, parent.pid, parent.fault_ep);
    child.entry = parent.entry;
    child.brk = parent.brk;
    child.heap_start = parent.heap_start;
    child.heap_mapped_end = parent.heap_mapped_end;
    child.elf_phdr = parent.elf_phdr;
    child.elf_phent = parent.elf_phent;
    child.elf_phnum = parent.elf_phnum;
    child.exec_path = parent.exec_path;
    child.exec_path_len = parent.exec_path_len;
    if parent.sparse_reserved != 0 {
        let _ = reserve_sparse_eager(&mut child, parent.sparse_reserved);
    }
    child.cwd = parent.cwd;
    child.cwd_len = parent.cwd_len;
    child.cwd_inode = parent.cwd_inode;
    child.fds = parent.fds;
    child.fd_serial = parent.fd_serial;
    if !vfs_retain_cwd(child.cwd_inode) {
        release_all_sparse_eager(&mut child);
        clear_process_mappings(alloc, child.pid);
        destroy_child_objects(alloc, &child);
        return -(ENOMEM as i64);
    }
    if !retain_fd_refs(&child) {
        let _ = vfs_release_cwd(child.cwd_inode);
        release_all_sparse_eager(&mut child);
        clear_process_mappings(alloc, child.pid);
        destroy_child_objects(alloc, &child);
        return -(ENOMEM as i64);
    }
    clone_address_space(alloc, &parent, &child);

    let mut ctx = read_user_context(parent.tcb);
    arch::set_user_context_pc(&mut ctx, arch::resumed_fault_pc(mrs));
    arch::set_user_context_return_value(&mut ctx, 0);
    write_user_context(child.tcb, &ctx, true);
    procs[slot] = child;

    info!("linux-compat: fork parent={} child={}", parent.pid, pid);
    pid as i64
}

pub(crate) fn sys_exit(
    alloc: &mut Allocator,
    procs: &mut [TaskStruct; MAX_PROCS],
    proc_idx: usize,
    status: i32,
) -> SyscallResult {
    let pid = procs[proc_idx].pid;
    info!("linux-compat: exit({}) pid={}", status, pid);
    if procs[proc_idx].parent_pid == 0 {
        halt_loop();
    }
    make_zombie_process(alloc, procs, proc_idx, linux_exit_status(status));
    SyscallResult::Stop
}

pub(crate) fn sys_wait4(
    alloc: &mut Allocator,
    procs: &mut [TaskStruct; MAX_PROCS],
    proc_idx: usize,
    wait_pid: i64,
    status_ptr: u64,
    options: u32,
    _rusage: u64,
    mrs: &[u64; 64],
) -> SyscallResult {
    wait_common(alloc, procs, proc_idx, wait_pid, status_ptr, options, mrs)
}

pub(crate) fn sys_waitid(
    alloc: &mut Allocator,
    procs: &mut [TaskStruct; MAX_PROCS],
    proc_idx: usize,
    which: u32,
    pid: i64,
    infop: u64,
    options: u32,
    mrs: &[u64; 64],
) -> SyscallResult {
    let wait_pid = match which {
        P_ALL => -1,
        P_PID => pid,
        _ => return SyscallResult::err(EINVAL),
    };
    match wait_common(alloc, procs, proc_idx, wait_pid, 0, options, mrs) {
        SyscallResult::Reply(ret) if ret > 0 && infop != 0 => {
            let mut info = [0u8; 128];
            write_i32(&mut info, 0, SIGCHLD as i32);
            write_i32(&mut info, 4, CLD_EXITED);
            write_i32(&mut info, 16, ret as i32);
            if !copy_to_child(alloc, &procs[proc_idx], infop, &info) {
                return SyscallResult::err(EFAULT);
            }
            SyscallResult::Reply(0)
        }
        other => other,
    }
}

fn wait_common(
    alloc: &mut Allocator,
    procs: &mut [TaskStruct; MAX_PROCS],
    proc_idx: usize,
    wait_pid: i64,
    status_ptr: u64,
    options: u32,
    mrs: &[u64; 64],
) -> SyscallResult {
    let parent_pid = procs[proc_idx].pid;
    let mut has_child = false;
    for i in 0..MAX_PROCS {
        if procs[i].parent_pid != parent_pid || procs[i].state == PROC_UNUSED {
            continue;
        }
        if wait_pid > 0 && procs[i].pid != wait_pid as u64 {
            continue;
        }
        has_child = true;
        if procs[i].state != PROC_ZOMBIE {
            continue;
        }
        let pid = procs[i].pid;
        let status = procs[i].exit_status;
        if status_ptr != 0 {
            let mut out = [0u8; 4];
            write_i32(&mut out, 0, status);
            if !copy_to_child(alloc, &procs[proc_idx], status_ptr, &out) {
                return SyscallResult::err(EFAULT);
            }
        }
        reap_process(alloc, &mut procs[i]);
        return SyscallResult::Reply(pid as i64);
    }
    if !has_child {
        return SyscallResult::err(ECHILD);
    }
    if options & WNOHANG != 0 {
        return SyscallResult::Reply(0);
    }

    let (reply_slot, reply_mrs) = save_blocked_reply(mrs);
    procs[proc_idx].state = PROC_WAITING;
    procs[proc_idx].wait_status_ptr = status_ptr;
    procs[proc_idx].wait_pid = wait_pid;
    procs[proc_idx].wait_options = options;
    procs[proc_idx].wait_reply_slot = reply_slot;
    procs[proc_idx].wait_reply_mrs = reply_mrs;
    SyscallResult::Block
}

pub(crate) fn sys_kill(
    alloc: &mut Allocator,
    procs: &mut [TaskStruct; MAX_PROCS],
    pid: i64,
    _sig: u64,
) -> i64 {
    if pid <= 0 {
        return -(ESRCH as i64);
    }
    for i in 0..MAX_PROCS {
        if procs[i].pid == pid as u64 && procs[i].state != PROC_UNUSED {
            call_checked(procs[i].tcb, LABEL_TCB_SUSPEND, &[], &[]);
            make_zombie_process(alloc, procs, i, linux_exit_status(1));
            return 0;
        }
    }
    -(ESRCH as i64)
}

pub(crate) fn sys_getpid(child: &TaskStruct) -> i64 {
    child.pid as i64
}

pub(crate) fn sys_getppid(child: &TaskStruct) -> i64 {
    child.parent_pid as i64
}

pub(crate) fn sys_gettid(child: &TaskStruct) -> i64 {
    child.pid as i64
}

pub(crate) fn sys_set_tid_address(child: &mut TaskStruct, addr: u64) -> i64 {
    child.tid_address = addr;
    child.pid as i64
}

pub(crate) fn sys_uname(alloc: &mut Allocator, child: &TaskStruct, dest: u64) -> SyscallResult {
    if dest == 0 {
        return SyscallResult::err(EFAULT);
    }
    let mut un = LinuxUtsname {
        sysname: [0; UTS_LEN],
        nodename: [0; UTS_LEN],
        release: [0; UTS_LEN],
        version: [0; UTS_LEN],
        machine: [0; UTS_LEN],
        domainname: [0; UTS_LEN],
    };
    copy_cstr(&mut un.sysname, b"Linux");
    copy_cstr(&mut un.nodename, b"linux-compat");
    copy_cstr(&mut un.release, b"6.1.0-linux-compat");
    copy_cstr(&mut un.version, b"#1 SMP");
    copy_cstr(&mut un.machine, crate::arch::current::UTS_MACHINE);
    copy_cstr(&mut un.domainname, b"(none)");
    let bytes = unsafe {
        core::slice::from_raw_parts(
            core::ptr::addr_of!(un) as *const u8,
            core::mem::size_of::<LinuxUtsname>(),
        )
    };
    if !copy_to_child(alloc, child, dest, bytes) {
        return SyscallResult::err(EFAULT);
    }
    SyscallResult::Reply(0)
}

fn copy_cstr(dst: &mut [u8], src: &[u8]) {
    let n = core::cmp::min(src.len(), dst.len().saturating_sub(1));
    dst[..n].copy_from_slice(&src[..n]);
}

pub(crate) fn fault_kill(
    alloc: &mut Allocator,
    procs: &mut [TaskStruct; MAX_PROCS],
    proc_idx: usize,
    label: u64,
) -> SyscallResult {
    let pid = procs[proc_idx].pid;
    warn!("linux-compat: fault kill pid={} label={}", pid, label);
    if procs[proc_idx].parent_pid == 0 {
        halt_loop();
    }
    call_checked(procs[proc_idx].tcb, LABEL_TCB_SUSPEND, &[], &[]);
    make_zombie_process(alloc, procs, proc_idx, linux_exit_status(1));
    SyscallResult::Stop
}

fn make_zombie_process(
    alloc: &mut Allocator,
    procs: &mut [TaskStruct; MAX_PROCS],
    proc_idx: usize,
    status: i32,
) {
    let pid = procs[proc_idx].pid;
    if procs[proc_idx].tid_address != 0 {
        let _ = copy_to_child(
            alloc,
            &procs[proc_idx],
            procs[proc_idx].tid_address,
            &[0; 4],
        );
    }
    release_all_sparse_eager(&mut procs[proc_idx]);
    close_all_fds(&mut procs[proc_idx]);
    release_cwd_ref(&mut procs[proc_idx]);
    drop_blocked_reply_caps(&mut procs[proc_idx]);
    procs[proc_idx].state = PROC_ZOMBIE;
    procs[proc_idx].exit_status = status;
    pump_vfs_waiters(alloc, procs);
    reparent_children(alloc, procs, pid);
    reply_waiting_parent(alloc, procs, proc_idx);
}

fn reply_waiting_parent(
    alloc: &mut Allocator,
    procs: &mut [TaskStruct; MAX_PROCS],
    child_idx: usize,
) {
    let parent_pid = procs[child_idx].parent_pid;
    let child_pid = procs[child_idx].pid;
    let status = procs[child_idx].exit_status;
    for i in 0..MAX_PROCS {
        if procs[i].pid != parent_pid || procs[i].state != PROC_WAITING {
            continue;
        }
        if procs[i].wait_pid > 0 && procs[i].wait_pid as u64 != child_pid {
            continue;
        }

        let parent = procs[i];
        let mut ret = child_pid as i64;
        if parent.wait_status_ptr != 0 {
            let mut out = [0u8; 4];
            write_i32(&mut out, 0, status);
            if !copy_to_child(alloc, &parent, parent.wait_status_ptr, &out) {
                ret = -(EFAULT as i64);
            }
        }

        let mut reply_mrs = parent.wait_reply_mrs;
        arch::set_syscall_return_value(&mut reply_mrs, ret as u64);
        if ret >= 0 {
            reap_process(alloc, &mut procs[child_idx]);
        }
        reply_caps::send_and_release(
            parent.wait_reply_slot,
            msg_info(0, 0, 0, arch::FAULT_REPLY_WORDS as u64),
            &reply_mrs,
        );
        procs[i].state = PROC_RUNNABLE;
        clear_wait_block(&mut procs[i]);
        return;
    }
}

fn reparent_children(alloc: &mut Allocator, procs: &mut [TaskStruct; MAX_PROCS], exiting_pid: u64) {
    let mut i = 0;
    while i < MAX_PROCS {
        if procs[i].parent_pid == exiting_pid && procs[i].state != PROC_UNUSED {
            procs[i].parent_pid = 1;
            procs[i].reparented_to_init = true;
            if procs[i].state == PROC_ZOMBIE {
                reply_waiting_parent(alloc, procs, i);
            }
        }
        i += 1;
    }
}

fn reap_process(alloc: &mut Allocator, child: &mut TaskStruct) {
    release_all_sparse_eager(child);
    clear_process_mappings(alloc, child.pid);
    destroy_child_objects(alloc, child);
    *child = TaskStruct::empty();
}

fn find_free_proc(procs: &[TaskStruct; MAX_PROCS]) -> Option<usize> {
    for i in 0..MAX_PROCS {
        if procs[i].state == PROC_UNUSED {
            return Some(i);
        }
    }
    None
}
