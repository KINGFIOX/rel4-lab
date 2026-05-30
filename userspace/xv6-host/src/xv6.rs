use core::cmp::min;

use crate::allocator::Allocator;
use crate::child::{
    clear_process_mappings, clone_address_space, copy_cstr_from_child, copy_from_child,
    copy_to_child, create_child, destroy_child_objects, is_child_page_mapped, load_elf,
    map_fresh_child_page, map_stack, mapping_slots_available, read_user_context,
    reset_process_mappings, unmap_child_range, write_user_context,
};
use crate::consts::*;
use crate::sel4::{call_checked, msg_info, sel4_send, sel4_yield};
use crate::types::{Child, DirEntry, FdEntry, FsNode};
use crate::util::*;

static mut TICKS: u64 = 0;
static mut PIPES: [Pipe; MAX_PIPES] = [Pipe::closed(); MAX_PIPES];
static mut OPEN_FILES: [OpenFile; MAX_OPEN_FILES] = [OpenFile::closed(); MAX_OPEN_FILES];
static mut NEXT_PID: u64 = 2;
static mut CONSOLE_INPUT_POS: usize = 0;
static mut FS_NODES: [FsNode; MAX_FS_NODES] = [FsNode::empty(); MAX_FS_NODES];
static mut DIR_ENTRIES: [DirEntry; MAX_DIR_ENTRIES] = [DirEntry::empty(); MAX_DIR_ENTRIES];
static mut FILE_BLOCKS: [[u8; FS_BLOCK_SIZE]; MAX_FILE_BLOCKS] =
    [[0; FS_BLOCK_SIZE]; MAX_FILE_BLOCKS];
static mut FILE_BLOCK_USED: [bool; MAX_FILE_BLOCKS] = [false; MAX_FILE_BLOCKS];
static mut NEXT_INO: u32 = 4;

pub(crate) enum SyscallResult {
    Reply(i64),
    ReplyFrame([u64; 11]),
    Block,
    Stop,
}

#[derive(Copy, Clone)]
struct Pipe {
    buf: [u8; PIPE_BUF],
    read_pos: usize,
    len: usize,
    readers: usize,
    writers: usize,
}

impl Pipe {
    const fn closed() -> Self {
        Self {
            buf: [0; PIPE_BUF],
            read_pos: 0,
            len: 0,
            readers: 0,
            writers: 0,
        }
    }
}

#[derive(Copy, Clone)]
struct OpenFile {
    used: bool,
    refs: u16,
    offset: usize,
}

impl OpenFile {
    const fn closed() -> Self {
        Self {
            used: false,
            refs: 0,
            offset: 0,
        }
    }
}

pub(crate) fn tick() {
    unsafe {
        TICKS = TICKS.wrapping_add(1);
    }
}

pub(crate) fn handle_xv6_syscall(
    alloc: &mut Allocator,
    procs: &mut [Child; MAX_PROCS],
    proc_idx: usize,
    mrs: &[u64; 64],
) -> SyscallResult {
    let sysno = mrs[10];
    let a0 = mrs[3];
    let a1 = mrs[4];
    let a2 = mrs[5];
    tick();

    let ret = match sysno {
        SYS_FORK => return SyscallResult::Reply(sys_fork(alloc, procs, proc_idx, mrs)),
        SYS_EXIT => {
            return sys_exit(alloc, procs, proc_idx, a0 as i32);
        }
        SYS_WAIT => return sys_wait(alloc, procs, proc_idx, a0, mrs),
        SYS_WRITE => {
            return sys_write(
                alloc,
                &mut procs[proc_idx],
                a0 as usize,
                a1,
                a2 as usize,
                mrs,
            );
        }
        SYS_READ => {
            let result = sys_read(&mut procs[proc_idx], a0 as usize, a1, a2 as usize);
            resume_pipe_writers(procs);
            return result;
        }
        SYS_OPEN => sys_open(&mut procs[proc_idx], a0, a1 as u32),
        SYS_CLOSE => sys_close(&mut procs[proc_idx], a0 as usize),
        SYS_DUP => sys_dup(&mut procs[proc_idx], a0 as usize),
        SYS_FSTAT => sys_fstat(&procs[proc_idx], a0 as usize, a1),
        SYS_SBRK => sys_sbrk(alloc, &mut procs[proc_idx], a0 as i64, a1),
        SYS_GETPID => procs[proc_idx].pid as i64,
        SYS_UPTIME => unsafe { TICKS as i64 },
        SYS_PAUSE => {
            unsafe { sel4_yield() };
            0
        }
        SYS_KILL => sys_kill(procs, a0 as i64),
        SYS_CHDIR => sys_chdir(&mut procs[proc_idx], a0),
        SYS_PIPE => sys_pipe(&mut procs[proc_idx], a0),
        SYS_MKNOD => sys_mknod(&procs[proc_idx], a0),
        SYS_EXEC => return sys_exec(alloc, &mut procs[proc_idx], a0, a1),
        SYS_UNLINK => sys_unlink(&procs[proc_idx], a0),
        SYS_LINK => sys_link(&procs[proc_idx], a0, a1),
        SYS_MKDIR => sys_mkdir(&procs[proc_idx], a0),
        _ => -1,
    };
    SyscallResult::Reply(ret)
}

pub(crate) fn handle_xv6_fault(
    alloc: &mut Allocator,
    procs: &mut [Child; MAX_PROCS],
    proc_idx: usize,
    label: u64,
    mrs: &[u64; 64],
) -> SyscallResult {
    if label == FAULT_VM_FAULT {
        let fault_addr = mrs[1];
        let fsr = mrs[3];
        if handle_lazy_page_fault(alloc, &mut procs[proc_idx], fault_addr, fsr) {
            return SyscallResult::ReplyFrame([0; 11]);
        }
    }
    fault_kill(alloc, procs, proc_idx, label)
}

fn sys_write(
    alloc: &mut Allocator,
    child: &mut Child,
    fd: usize,
    buf: u64,
    len: usize,
    mrs: &[u64; 64],
) -> SyscallResult {
    if len == 0 {
        return SyscallResult::Reply(0);
    }
    let mut scratch = [0u8; 128];
    let mut done = 0usize;
    while done < len {
        let n = min(scratch.len(), len - done);
        if !copy_from_child(child, buf + done as u64, &mut scratch[..n]) {
            return SyscallResult::Reply(-1);
        }
        let wrote = {
            if fd >= MAX_FD {
                return SyscallResult::Reply(-1);
            }
            match child.fds[fd].kind {
                FD_CONSOLE if child.fds[fd].writable => {
                    for b in &scratch[..n] {
                        putchar(*b);
                    }
                    n
                }
                FD_PIPE_WRITE => unsafe { pipe_write(child.fds[fd].aux, &scratch[..n]) },
                FD_FS_FILE if child.fds[fd].writable => {
                    let Some(offset) = fd_offset(child.fds[fd]) else {
                        return SyscallResult::Reply(-1);
                    };
                    match fs_write(child.fds[fd].aux, offset, &scratch[..n]) {
                        Some(written) => written,
                        None if done == 0 => return SyscallResult::Reply(-1),
                        None => 0,
                    }
                }
                _ => return SyscallResult::Reply(-1),
            }
        };
        if wrote == 0 {
            if fd < MAX_FD
                && child.fds[fd].kind == FD_PIPE_WRITE
                && unsafe { pipe_has_readers(child.fds[fd].aux) }
            {
                block_pipe_writer(alloc, child, fd, buf, len, done, mrs);
                return SyscallResult::Block;
            }
            if done == 0 {
                return SyscallResult::Reply(-1);
            }
            break;
        }
        if fd < MAX_FD && child.fds[fd].kind == FD_FS_FILE {
            advance_fd_offset(child.fds[fd], wrote);
        }
        done += wrote;
    }
    SyscallResult::Reply(done as i64)
}

fn block_pipe_writer(
    alloc: &mut Allocator,
    child: &mut Child,
    fd: usize,
    buf: u64,
    len: usize,
    done: usize,
    mrs: &[u64; 64],
) {
    let reply_slot = alloc.alloc_slot();
    call_checked(
        ROOT_CNODE,
        LABEL_CNODE_SAVE_CALLER,
        &[],
        &[reply_slot, ROOT_CNODE_DEPTH],
    );
    let mut reply_mrs = [0u64; 11];
    reply_mrs.copy_from_slice(&mrs[..11]);
    reply_mrs[0] = mrs[0].wrapping_add(4);

    child.state = PROC_PIPE_WRITE;
    child.pipe_reply_slot = reply_slot;
    child.pipe_reply_mrs = reply_mrs;
    child.pipe_fd = fd;
    child.pipe_buf = buf;
    child.pipe_len = len;
    child.pipe_done = done;
}

fn resume_pipe_writers(procs: &mut [Child; MAX_PROCS]) {
    for child in procs.iter_mut() {
        if child.state != PROC_PIPE_WRITE {
            continue;
        }
        let Some(ret) = resume_pipe_writer(child) else {
            continue;
        };
        let mut reply_mrs = child.pipe_reply_mrs;
        reply_mrs[3] = ret as u64;
        unsafe {
            sel4_send(child.pipe_reply_slot, msg_info(0, 0, 0, 11), &reply_mrs);
        }
        child.state = PROC_RUNNABLE;
        child.pipe_reply_slot = 0;
        child.pipe_reply_mrs = [0; 11];
        child.pipe_fd = 0;
        child.pipe_buf = 0;
        child.pipe_len = 0;
        child.pipe_done = 0;
    }
}

fn resume_pipe_writer(child: &mut Child) -> Option<i64> {
    let fd = child.pipe_fd;
    if fd >= MAX_FD || child.fds[fd].kind != FD_PIPE_WRITE {
        return Some(-1);
    }
    let pipe_idx = child.fds[fd].aux;
    let mut scratch = [0u8; 128];
    while child.pipe_done < child.pipe_len {
        let n = min(scratch.len(), child.pipe_len - child.pipe_done);
        if !copy_from_child(
            child,
            child.pipe_buf + child.pipe_done as u64,
            &mut scratch[..n],
        ) {
            return Some(-1);
        }
        let wrote = unsafe { pipe_write(pipe_idx, &scratch[..n]) };
        if wrote == 0 {
            if unsafe { pipe_has_readers(pipe_idx) } {
                return None;
            }
            return Some(-1);
        }
        child.pipe_done += wrote;
    }
    Some(child.pipe_done as i64)
}

fn sys_read(child: &mut Child, fd: usize, dst: u64, len: usize) -> SyscallResult {
    if fd >= MAX_FD || len == 0 {
        return SyscallResult::Reply(if fd < MAX_FD { 0 } else { -1 });
    }
    let entry = child.fds[fd];
    match entry.kind {
        FD_CONSOLE => sys_read_console(child, dst, len),
        FD_FS_FILE if entry.readable => SyscallResult::Reply(fs_read_file(child, fd, dst, len)),
        FD_FS_DIR if entry.readable => SyscallResult::Reply(fs_read_dir(child, fd, dst, len)),
        FD_PIPE_READ => SyscallResult::Reply(unsafe { pipe_read(child, entry.aux, dst, len) }),
        _ => SyscallResult::Reply(-1),
    }
}

fn sys_read_console(child: &Child, dst: u64, len: usize) -> SyscallResult {
    if CONSOLE_INPUT.is_empty() {
        return SyscallResult::Block;
    }
    unsafe {
        if CONSOLE_INPUT_POS >= CONSOLE_INPUT.len() {
            return SyscallResult::Reply(0);
        }
        let n = min(len, CONSOLE_INPUT.len() - CONSOLE_INPUT_POS);
        let chunk = &CONSOLE_INPUT[CONSOLE_INPUT_POS..CONSOLE_INPUT_POS + n];
        if !copy_to_child(child, dst, chunk) {
            return SyscallResult::Reply(-1);
        }
        for b in chunk {
            putchar(*b);
        }
        CONSOLE_INPUT_POS += n;
        SyscallResult::Reply(n as i64)
    }
}

fn sys_open(child: &mut Child, path_ptr: u64, flags: u32) -> i64 {
    let mut path = [0u8; 128];
    let Some(len) = copy_cstr_from_child(child, path_ptr, &mut path) else {
        return -1;
    };
    let path = &path[..len];
    let wants_write = flags & (O_WRONLY | O_RDWR | O_CREATE | O_TRUNC) != 0;
    let readable = flags & O_WRONLY == 0 || flags & O_RDWR != 0;
    let writable = flags & (O_WRONLY | O_RDWR) != 0;

    let node = match lookup_path(child, path) {
        Some(node) => node,
        None if flags & O_CREATE != 0 => {
            let Some((parent, name, name_len)) = lookup_parent(child, path) else {
                return -1;
            };
            let Some(node) = create_fs_node(parent, &name[..name_len], FS_FILE) else {
                return -1;
            };
            node
        }
        None => return -1,
    };

    let kind = unsafe { FS_NODES[node].kind };
    if kind == FS_CONSOLE {
        return alloc_fd(child, FD_CONSOLE, 0, true, true);
    }
    if kind == FS_DIR && wants_write {
        return -1;
    }
    if (kind == FS_README || kind == FS_EXEC) && wants_write {
        return -1;
    }
    if kind == FS_FILE && flags & O_TRUNC != 0 && writable {
        unsafe {
            truncate_file(node);
        }
    }

    let fd_kind = if kind == FS_DIR {
        FD_FS_DIR
    } else {
        FD_FS_FILE
    };
    alloc_fd(child, fd_kind, node, readable, writable)
}

fn sys_close(child: &mut Child, fd: usize) -> i64 {
    if fd >= MAX_FD || child.fds[fd].kind == FD_CLOSED {
        return -1;
    }
    close_fd(child, fd);
    0
}

fn close_fd(child: &mut Child, fd: usize) {
    let entry = child.fds[fd];
    child.fds[fd] = FdEntry::closed();
    if !release_open_file(entry) {
        return;
    }
    unsafe {
        match entry.kind {
            FD_PIPE_READ if entry.aux < MAX_PIPES && PIPES[entry.aux].readers > 0 => {
                PIPES[entry.aux].readers -= 1;
            }
            FD_PIPE_WRITE if entry.aux < MAX_PIPES && PIPES[entry.aux].writers > 0 => {
                PIPES[entry.aux].writers -= 1;
            }
            FD_FS_FILE | FD_FS_DIR if entry.aux < MAX_FS_NODES => {
                if FS_NODES[entry.aux].open_refs > 0 {
                    FS_NODES[entry.aux].open_refs -= 1;
                }
                maybe_free_node(entry.aux);
            }
            _ => {}
        }
    }
}

fn sys_dup(child: &mut Child, fd: usize) -> i64 {
    if fd >= MAX_FD {
        return -1;
    }
    let entry = child.fds[fd];
    if entry.kind == FD_CLOSED {
        return -1;
    }
    for i in 0..MAX_FD {
        if child.fds[i].kind == FD_CLOSED {
            child.fds[i] = entry;
            retain_open_file(entry);
            return i as i64;
        }
    }
    -1
}

fn sys_fstat(child: &Child, fd: usize, dst: u64) -> i64 {
    if fd >= MAX_FD {
        return -1;
    }
    let entry = child.fds[fd];
    let (typ, ino, size) = match entry.kind {
        FD_CONSOLE => (T_DEVICE, CONSOLE_INO, 0u64),
        FD_FS_FILE | FD_FS_DIR => fs_stat(entry.aux),
        FD_PIPE_READ | FD_PIPE_WRITE => (T_FILE, 4 + entry.aux as u32, 0u64),
        _ => return -1,
    };
    let mut st = [0u8; 24];
    write_i32(&mut st, 0, 1);
    write_u32(&mut st, 4, ino);
    write_u16(&mut st, 8, typ);
    write_u16(&mut st, 10, 1);
    write_u64_bytes(&mut st, 16, size);
    if !copy_to_child(child, dst, &st) {
        return -1;
    }
    0
}

fn sys_sbrk(alloc: &mut Allocator, child: &mut Child, increment: i64, mode: u64) -> i64 {
    let old = child.brk;
    let new_brk = if increment >= 0 {
        old.saturating_add(increment as u64)
    } else {
        let decrement = (-increment) as u64;
        if decrement > old {
            old
        } else {
            old - decrement
        }
    };
    if new_brk > CHILD_HEAP_LIMIT {
        return -1;
    }
    if mode == SBRK_EAGER && new_brk > CHILD_EAGER_HEAP_LIMIT {
        return -1;
    }
    if mode != SBRK_EAGER && mode != SBRK_LAZY {
        return -1;
    }
    if new_brk < old {
        let new_mapped_end = align_up(new_brk);
        unmap_child_range(alloc, child.pid, new_mapped_end, align_up(old));
        child.heap_mapped_end = child.heap_mapped_end.min(new_mapped_end);
    }
    if new_brk > child.heap_mapped_end {
        let target_end = align_up(new_brk);
        let first_page = align_up(child.heap_mapped_end);
        let needed = ((target_end.saturating_sub(first_page)) / PAGE_SIZE) as usize;
        if needed <= SBRK_EAGER_MAP_LIMIT {
            let available = mapping_slots_available();
            if needed > available.saturating_sub(SBRK_MAPPING_HEADROOM) {
                return -1;
            }
            let mut page = align_up(child.heap_mapped_end);
            while page < target_end {
                map_fresh_child_page(alloc, child, page, true, false);
                page += PAGE_SIZE;
            }
            child.heap_mapped_end = target_end;
        }
    }
    child.brk = new_brk;
    old as i64
}

fn handle_lazy_page_fault(
    alloc: &mut Allocator,
    child: &mut Child,
    fault_addr: u64,
    fsr: u64,
) -> bool {
    if fault_addr < child.heap_start || fault_addr >= child.brk || fault_addr >= CHILD_HEAP_LIMIT {
        return false;
    }
    if is_child_page_mapped(child, fault_addr) {
        return false;
    }
    if fsr != 5 && fsr != 7 {
        return false;
    }
    if mapping_slots_available() <= SBRK_MAPPING_HEADROOM {
        return false;
    }
    map_fresh_child_page(alloc, child, fault_addr, true, false);
    true
}

fn fault_kill(
    alloc: &mut Allocator,
    procs: &mut [Child; MAX_PROCS],
    proc_idx: usize,
    label: u64,
) -> SyscallResult {
    let pid = procs[proc_idx].pid;
    log("xv6-host: fault kill pid=");
    print_u64(pid);
    log(" label=");
    print_u64(label);
    log("\n");
    if procs[proc_idx].parent_pid == 0 {
        halt_loop();
    }
    call_checked(procs[proc_idx].tcb, LABEL_TCB_SUSPEND, &[], &[]);
    close_all_fds(&mut procs[proc_idx]);
    reparent_children(alloc, procs, pid);
    procs[proc_idx].state = PROC_ZOMBIE;
    procs[proc_idx].exit_status = -1;
    if procs[proc_idx].reparented_to_init && !ROOT_IS_INIT {
        reap_process(alloc, &mut procs[proc_idx]);
        return SyscallResult::Stop;
    }
    reply_waiting_parent(alloc, procs, proc_idx);
    SyscallResult::Stop
}

fn sys_fork(
    alloc: &mut Allocator,
    procs: &mut [Child; MAX_PROCS],
    parent_idx: usize,
    mrs: &[u64; 64],
) -> i64 {
    let Some(slot) = find_free_proc(procs) else {
        return -1;
    };
    let parent = procs[parent_idx];
    let pid = unsafe {
        let pid = NEXT_PID;
        NEXT_PID = NEXT_PID.wrapping_add(1);
        pid
    };

    let mut child = create_child(alloc, slot, pid, parent.pid, parent.fault_ep);
    child.entry = parent.entry;
    child.brk = parent.brk;
    child.heap_start = parent.heap_start;
    child.heap_mapped_end = parent.heap_mapped_end;
    child.fds = parent.fds;
    child.cwd = parent.cwd;
    retain_fd_refs(&child);
    clone_address_space(alloc, &parent, &child);

    let mut ctx = read_user_context(parent.tcb);
    ctx[0] = mrs[0].wrapping_add(4);
    ctx[16] = 0;
    write_user_context(child.tcb, &ctx, true);
    procs[slot] = child;

    log("xv6-host: fork parent=");
    print_u64(parent.pid);
    log(" child=");
    print_u64(pid);
    log("\n");
    pid as i64
}

fn sys_exit(
    alloc: &mut Allocator,
    procs: &mut [Child; MAX_PROCS],
    proc_idx: usize,
    status: i32,
) -> SyscallResult {
    let pid = procs[proc_idx].pid;
    log("xv6-host: exit(");
    print_i64(status as i64);
    log(") pid=");
    print_u64(pid);
    log("\n");
    if procs[proc_idx].parent_pid == 0 {
        halt_loop();
    }
    close_all_fds(&mut procs[proc_idx]);
    reparent_children(alloc, procs, pid);
    procs[proc_idx].state = PROC_ZOMBIE;
    procs[proc_idx].exit_status = status;
    if procs[proc_idx].reparented_to_init && !ROOT_IS_INIT {
        reap_process(alloc, &mut procs[proc_idx]);
        return SyscallResult::Stop;
    }
    reply_waiting_parent(alloc, procs, proc_idx);
    SyscallResult::Stop
}

fn sys_wait(
    alloc: &mut Allocator,
    procs: &mut [Child; MAX_PROCS],
    proc_idx: usize,
    status_ptr: u64,
    mrs: &[u64; 64],
) -> SyscallResult {
    let parent_pid = procs[proc_idx].pid;
    let mut has_child = false;
    for i in 0..MAX_PROCS {
        if procs[i].parent_pid != parent_pid || procs[i].state == PROC_UNUSED {
            continue;
        }
        if parent_pid == 1 && procs[i].reparented_to_init && !ROOT_IS_INIT {
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
            if !copy_to_child(&procs[proc_idx], status_ptr, &out) {
                return SyscallResult::Reply(-1);
            }
        }
        reap_process(alloc, &mut procs[i]);
        return SyscallResult::Reply(pid as i64);
    }
    if !has_child {
        return SyscallResult::Reply(-1);
    }

    let reply_slot = alloc.alloc_slot();
    call_checked(
        ROOT_CNODE,
        LABEL_CNODE_SAVE_CALLER,
        &[],
        &[reply_slot, ROOT_CNODE_DEPTH],
    );
    let mut reply_mrs = [0u64; 11];
    reply_mrs.copy_from_slice(&mrs[..11]);
    reply_mrs[0] = mrs[0].wrapping_add(4);
    procs[proc_idx].state = PROC_WAITING;
    procs[proc_idx].wait_status_ptr = status_ptr;
    procs[proc_idx].wait_reply_slot = reply_slot;
    procs[proc_idx].wait_reply_mrs = reply_mrs;
    SyscallResult::Block
}

fn reply_waiting_parent(alloc: &mut Allocator, procs: &mut [Child; MAX_PROCS], child_idx: usize) {
    let parent_pid = procs[child_idx].parent_pid;
    let child_pid = procs[child_idx].pid;
    let status = procs[child_idx].exit_status;
    for i in 0..MAX_PROCS {
        if procs[i].pid != parent_pid || procs[i].state != PROC_WAITING {
            continue;
        }

        let parent = procs[i];
        let mut ret = child_pid as i64;
        if parent.wait_status_ptr != 0 {
            let mut out = [0u8; 4];
            write_i32(&mut out, 0, status);
            if !copy_to_child(&parent, parent.wait_status_ptr, &out) {
                ret = -1;
            }
        }

        let mut reply_mrs = parent.wait_reply_mrs;
        reply_mrs[3] = ret as u64;
        unsafe {
            sel4_send(parent.wait_reply_slot, msg_info(0, 0, 0, 11), &reply_mrs);
        }
        procs[i].state = PROC_RUNNABLE;
        procs[i].wait_status_ptr = 0;
        procs[i].wait_reply_slot = 0;
        procs[i].wait_reply_mrs = [0; 11];
        if ret >= 0 {
            reap_process(alloc, &mut procs[child_idx]);
        }
        return;
    }
}

fn reparent_children(alloc: &mut Allocator, procs: &mut [Child; MAX_PROCS], exiting_pid: u64) {
    let mut i = 0;
    while i < MAX_PROCS {
        if procs[i].parent_pid == exiting_pid && procs[i].state != PROC_UNUSED {
            procs[i].parent_pid = 1;
            if !ROOT_IS_INIT {
                procs[i].reparented_to_init = true;
            }
            if procs[i].state == PROC_ZOMBIE {
                if ROOT_IS_INIT {
                    reply_waiting_parent(alloc, procs, i);
                } else {
                    reap_process(alloc, &mut procs[i]);
                }
            }
        }
        i += 1;
    }
}

fn reap_process(alloc: &mut Allocator, child: &mut Child) {
    clear_process_mappings(alloc, child.pid);
    destroy_child_objects(alloc, child);
    *child = Child::empty();
}

fn sys_kill(procs: &mut [Child; MAX_PROCS], pid: i64) -> i64 {
    if pid <= 0 {
        return -1;
    }
    for proc in procs.iter_mut() {
        if proc.pid == pid as u64 && proc.state != PROC_UNUSED {
            call_checked(proc.tcb, LABEL_TCB_SUSPEND, &[], &[]);
            close_all_fds(proc);
            proc.state = PROC_ZOMBIE;
            proc.exit_status = -1;
            return 0;
        }
    }
    -1
}

fn sys_exec(
    alloc: &mut Allocator,
    child: &mut Child,
    path_ptr: u64,
    argv_ptr: u64,
) -> SyscallResult {
    let mut path = [0u8; 128];
    let Some(path_len) = copy_cstr_from_child(child, path_ptr, &mut path) else {
        return SyscallResult::Reply(-1);
    };
    let name = basename(&path[..path_len]);
    let Some(image) = find_exec_image(name) else {
        return SyscallResult::Reply(-1);
    };

    let mut args = [[0u8; MAX_EXEC_ARG_LEN]; MAX_EXEC_ARGS];
    let mut arg_lens = [0usize; MAX_EXEC_ARGS];
    let Some(argc) = collect_exec_args(child, argv_ptr, &mut args, &mut arg_lens) else {
        return SyscallResult::Reply(-1);
    };

    reset_process_mappings(alloc, child.pid);
    load_elf(alloc, child, image.elf);
    map_stack(alloc, child);
    let Some((sp, argv_va)) = setup_exec_stack(child, &args, &arg_lens, argc) else {
        return SyscallResult::Reply(-1);
    };

    let mut ctx = [0u64; crate::child::USER_CONTEXT_WORDS];
    ctx[0] = child.entry;
    ctx[2] = sp;
    ctx[16] = argc as u64;
    ctx[17] = argv_va;
    write_user_context(child.tcb, &ctx, false);

    let mut reply = [0u64; 11];
    reply[0] = child.entry;
    reply[1] = sp;
    reply[2] = 0;
    reply[3] = argc as u64;
    reply[4] = argv_va;
    log("xv6-host: exec ");
    log_bytes(name);
    log(" pid=");
    print_u64(child.pid);
    log("\n");
    SyscallResult::ReplyFrame(reply)
}

fn find_exec_image(name: &[u8]) -> Option<&'static ExecImage> {
    for image in EXEC_IMAGES {
        if image.name == name {
            return Some(image);
        }
    }
    None
}

fn collect_exec_args(
    child: &Child,
    argv_ptr: u64,
    args: &mut [[u8; MAX_EXEC_ARG_LEN]; MAX_EXEC_ARGS],
    arg_lens: &mut [usize; MAX_EXEC_ARGS],
) -> Option<usize> {
    let mut argc = 0;
    loop {
        if argc >= MAX_EXEC_ARGS {
            return None;
        }
        let ptr = read_child_u64(child, argv_ptr + (argc as u64 * 8))?;
        if ptr == 0 {
            return Some(argc);
        }
        let len = copy_cstr_from_child(child, ptr, &mut args[argc])?;
        arg_lens[argc] = len;
        argc += 1;
    }
}

fn setup_exec_stack(
    child: &Child,
    args: &[[u8; MAX_EXEC_ARG_LEN]; MAX_EXEC_ARGS],
    arg_lens: &[usize; MAX_EXEC_ARGS],
    argc: usize,
) -> Option<(u64, u64)> {
    let mut sp = CHILD_STACK_TOP;
    let mut arg_ptrs = [0u64; MAX_EXEC_ARGS];
    for i in 0..argc {
        let len = arg_lens[i];
        sp = sp.checked_sub((len + 1) as u64)?;
        if !copy_to_child(child, sp, &args[i][..len]) {
            return None;
        }
        if !copy_to_child(child, sp + len as u64, &[0]) {
            return None;
        }
        arg_ptrs[i] = sp;
    }

    sp &= !0xf;
    sp = sp.checked_sub(8)?;
    if !write_child_u64(child, sp, 0) {
        return None;
    }
    for i in (0..argc).rev() {
        sp = sp.checked_sub(8)?;
        if !write_child_u64(child, sp, arg_ptrs[i]) {
            return None;
        }
    }
    let argv_va = sp;
    sp &= !0xf;
    Some((sp, argv_va))
}

fn read_child_u64(child: &Child, va: u64) -> Option<u64> {
    let mut bytes = [0u8; 8];
    if !copy_from_child(child, va, &mut bytes) {
        return None;
    }
    Some(read_u64(&bytes, 0))
}

fn write_child_u64(child: &Child, va: u64, value: u64) -> bool {
    let mut bytes = [0u8; 8];
    write_u64_bytes(&mut bytes, 0, value);
    copy_to_child(child, va, &bytes)
}

fn sys_chdir(child: &mut Child, path_ptr: u64) -> i64 {
    let mut path = [0u8; 128];
    let Some(len) = copy_cstr_from_child(child, path_ptr, &mut path) else {
        return -1;
    };
    let Some(node) = lookup_path(child, &path[..len]) else {
        return -1;
    };
    unsafe {
        if FS_NODES[node].kind != FS_DIR {
            return -1;
        }
    }
    child.cwd = node;
    0
}

fn sys_pipe(child: &mut Child, fds_ptr: u64) -> i64 {
    unsafe {
        let mut pipe_idx = MAX_PIPES;
        for i in 0..MAX_PIPES {
            if PIPES[i].readers == 0 && PIPES[i].writers == 0 {
                pipe_idx = i;
                break;
            }
        }
        if pipe_idx == MAX_PIPES {
            return -1;
        }

        let Some(read_fd) = find_free_fd(child) else {
            return -1;
        };
        let Some(read_file) = alloc_open_file() else {
            return -1;
        };
        child.fds[read_fd] = FdEntry {
            kind: FD_PIPE_READ,
            file: read_file,
            aux: pipe_idx,
            readable: true,
            writable: false,
        };
        let Some(write_fd) = find_free_fd(child) else {
            child.fds[read_fd] = FdEntry::closed();
            close_open_file(read_file);
            return -1;
        };
        let Some(write_file) = alloc_open_file() else {
            child.fds[read_fd] = FdEntry::closed();
            close_open_file(read_file);
            return -1;
        };

        PIPES[pipe_idx] = Pipe::closed();
        PIPES[pipe_idx].readers = 1;
        PIPES[pipe_idx].writers = 1;
        child.fds[write_fd] = FdEntry {
            kind: FD_PIPE_WRITE,
            file: write_file,
            aux: pipe_idx,
            readable: false,
            writable: true,
        };

        let mut out = [0u8; 8];
        write_i32(&mut out, 0, read_fd as i32);
        write_i32(&mut out, 4, write_fd as i32);
        if !copy_to_child(child, fds_ptr, &out) {
            close_fd(child, read_fd);
            close_fd(child, write_fd);
            return -1;
        }
        0
    }
}

fn sys_mknod(child: &Child, path_ptr: u64) -> i64 {
    let mut path = [0u8; 128];
    let Some(len) = copy_cstr_from_child(child, path_ptr, &mut path) else {
        return -1;
    };
    if basename(&path[..len]) == b"console" {
        0
    } else {
        -1
    }
}

pub(crate) fn init_fds(child: &mut Child) {
    unsafe {
        let mut i = 0;
        while i < MAX_FD {
            child.fds[i] = FdEntry::closed();
            i += 1;
        }
        let mut of = 0;
        while of < MAX_OPEN_FILES {
            OPEN_FILES[of] = OpenFile::closed();
            of += 1;
        }
        let Some(stdin_file) = alloc_open_file() else {
            halt_loop();
        };
        let Some(stdout_file) = alloc_open_file() else {
            halt_loop();
        };
        let Some(stderr_file) = alloc_open_file() else {
            halt_loop();
        };
        child.fds[0] = FdEntry {
            kind: FD_CONSOLE,
            file: stdin_file,
            aux: 0,
            readable: true,
            writable: true,
        };
        child.fds[1] = FdEntry {
            kind: FD_CONSOLE,
            file: stdout_file,
            aux: 0,
            readable: true,
            writable: true,
        };
        child.fds[2] = FdEntry {
            kind: FD_CONSOLE,
            file: stderr_file,
            aux: 0,
            readable: true,
            writable: true,
        };
        let mut p = 0;
        while p < MAX_PIPES {
            PIPES[p] = Pipe::closed();
            p += 1;
        }
        CONSOLE_INPUT_POS = 0;
        init_fs();
    }
}

fn alloc_fd(child: &mut Child, kind: u8, aux: usize, readable: bool, writable: bool) -> i64 {
    if let Some(i) = find_free_fd(child) {
        let Some(file) = alloc_open_file() else {
            return -1;
        };
        child.fds[i] = FdEntry {
            kind,
            file,
            aux,
            readable,
            writable,
        };
        open_backing(child.fds[i]);
        return i as i64;
    }
    -1
}

fn find_free_fd(child: &Child) -> Option<usize> {
    for i in 0..MAX_FD {
        if child.fds[i].kind == FD_CLOSED {
            return Some(i);
        }
    }
    None
}

fn find_free_proc(procs: &[Child; MAX_PROCS]) -> Option<usize> {
    for i in 0..MAX_PROCS {
        if procs[i].state == PROC_UNUSED {
            return Some(i);
        }
    }
    None
}

fn retain_fd_refs(child: &Child) {
    for entry in child.fds {
        retain_open_file(entry);
    }
}

fn alloc_open_file() -> Option<usize> {
    unsafe {
        let mut i = 0;
        while i < MAX_OPEN_FILES {
            if !OPEN_FILES[i].used {
                OPEN_FILES[i] = OpenFile {
                    used: true,
                    refs: 1,
                    offset: 0,
                };
                return Some(i);
            }
            i += 1;
        }
    }
    None
}

fn retain_open_file(entry: FdEntry) {
    unsafe {
        if entry.file < MAX_OPEN_FILES && OPEN_FILES[entry.file].used {
            OPEN_FILES[entry.file].refs = OPEN_FILES[entry.file].refs.saturating_add(1);
        }
    }
}

fn release_open_file(entry: FdEntry) -> bool {
    unsafe {
        if entry.file >= MAX_OPEN_FILES || !OPEN_FILES[entry.file].used {
            return false;
        }
        if OPEN_FILES[entry.file].refs > 1 {
            OPEN_FILES[entry.file].refs -= 1;
            return false;
        }
        OPEN_FILES[entry.file] = OpenFile::closed();
        true
    }
}

fn close_open_file(file: usize) {
    unsafe {
        if file < MAX_OPEN_FILES {
            OPEN_FILES[file] = OpenFile::closed();
        }
    }
}

fn fd_offset(entry: FdEntry) -> Option<usize> {
    unsafe {
        if entry.file < MAX_OPEN_FILES && OPEN_FILES[entry.file].used {
            Some(OPEN_FILES[entry.file].offset)
        } else {
            None
        }
    }
}

fn advance_fd_offset(entry: FdEntry, by: usize) {
    unsafe {
        if entry.file < MAX_OPEN_FILES && OPEN_FILES[entry.file].used {
            OPEN_FILES[entry.file].offset = OPEN_FILES[entry.file].offset.saturating_add(by);
        }
    }
}

fn open_backing(entry: FdEntry) {
    unsafe {
        match entry.kind {
            FD_PIPE_READ if entry.aux < MAX_PIPES => PIPES[entry.aux].readers += 1,
            FD_PIPE_WRITE if entry.aux < MAX_PIPES => PIPES[entry.aux].writers += 1,
            FD_FS_FILE | FD_FS_DIR if entry.aux < MAX_FS_NODES => {
                FS_NODES[entry.aux].open_refs = FS_NODES[entry.aux].open_refs.saturating_add(1);
            }
            _ => {}
        }
    }
}

fn close_all_fds(child: &mut Child) {
    for fd in 0..MAX_FD {
        if child.fds[fd].kind != FD_CLOSED {
            close_fd(child, fd);
        }
    }
}

fn init_fs() {
    unsafe {
        let mut i = 0;
        while i < MAX_FS_NODES {
            FS_NODES[i] = FsNode::empty();
            i += 1;
        }
        let mut d = 0;
        while d < MAX_DIR_ENTRIES {
            DIR_ENTRIES[d] = DirEntry::empty();
            d += 1;
        }
        let mut b = 0;
        while b < MAX_FILE_BLOCKS {
            FILE_BLOCK_USED[b] = false;
            b += 1;
        }
        NEXT_INO = 4;

        FS_NODES[FS_ROOT_NODE] = FsNode {
            used: true,
            kind: FS_DIR,
            ino: ROOT_INO,
            parent: FS_ROOT_NODE,
            nlink: 1,
            open_refs: 0,
            size: 0,
            exec_index: 0,
            blocks: [NO_FILE_BLOCK; MAX_FILE_BLOCK_REFS],
        };
        FS_NODES[FS_README_NODE] = FsNode {
            used: true,
            kind: FS_README,
            ino: README_INO,
            parent: FS_ROOT_NODE,
            nlink: 1,
            open_refs: 0,
            size: README_BYTES.len(),
            exec_index: 0,
            blocks: [NO_FILE_BLOCK; MAX_FILE_BLOCK_REFS],
        };
        FS_NODES[FS_CONSOLE_NODE] = FsNode {
            used: true,
            kind: FS_CONSOLE,
            ino: CONSOLE_INO,
            parent: FS_ROOT_NODE,
            nlink: 1,
            open_refs: 0,
            size: 0,
            exec_index: 0,
            blocks: [NO_FILE_BLOCK; MAX_FILE_BLOCK_REFS],
        };
        let _ = add_dir_entry(FS_ROOT_NODE, b"README", FS_README_NODE);
        let _ = add_dir_entry(FS_ROOT_NODE, b"console", FS_CONSOLE_NODE);
        init_exec_files();
    }
}

fn init_exec_files() {
    let mut i = 0usize;
    while i < EXEC_IMAGES.len() {
        let name = EXEC_IMAGES[i].name;
        if name.len() <= DIRSIZ && find_dir_entry(FS_ROOT_NODE, name).is_none() {
            if let Some(node) = create_fs_node(FS_ROOT_NODE, name, FS_EXEC) {
                unsafe {
                    FS_NODES[node].size = EXEC_IMAGES[i].elf.len();
                    FS_NODES[node].exec_index = i;
                }
            }
        }
        i += 1;
    }
}

fn fs_stat(node: usize) -> (u16, u32, u64) {
    unsafe {
        if node >= MAX_FS_NODES || !FS_NODES[node].used {
            return (T_FILE, 0, 0);
        }
        let typ = match FS_NODES[node].kind {
            FS_DIR => T_DIR,
            FS_CONSOLE => T_DEVICE,
            _ => T_FILE,
        };
        (typ, FS_NODES[node].ino, fs_node_size(node) as u64)
    }
}

fn fs_node_size(node: usize) -> usize {
    unsafe {
        match FS_NODES[node].kind {
            FS_README => README_BYTES.len(),
            FS_EXEC => EXEC_IMAGES[FS_NODES[node].exec_index].elf.len(),
            FS_DIR => dir_entry_count(node) * 16,
            _ => FS_NODES[node].size,
        }
    }
}

fn fs_read_file(child: &mut Child, fd: usize, dst: u64, len: usize) -> i64 {
    let node = child.fds[fd].aux;
    unsafe {
        if node >= MAX_FS_NODES || !FS_NODES[node].used {
            return -1;
        }
        let Some(offset) = fd_offset(child.fds[fd]) else {
            return -1;
        };
        let size = fs_node_size(node);
        if offset >= size {
            return 0;
        }
        let n = min(len, size - offset);
        let ok = match FS_NODES[node].kind {
            FS_README => copy_to_child(child, dst, &README_BYTES[offset..offset + n]),
            FS_EXEC => {
                let image = &EXEC_IMAGES[FS_NODES[node].exec_index].elf[offset..offset + n];
                copy_to_child(child, dst, image)
            }
            FS_FILE => fs_read_mutable_file(child, node, offset, dst, n),
            _ => false,
        };
        if !ok {
            return -1;
        }
        advance_fd_offset(child.fds[fd], n);
        n as i64
    }
}

fn fs_write(node: usize, offset: usize, src: &[u8]) -> Option<usize> {
    unsafe {
        if node >= MAX_FS_NODES || !FS_NODES[node].used || FS_NODES[node].kind != FS_FILE {
            return None;
        }
        if offset > FS_NODES[node].size || offset >= MAX_FILE_BYTES {
            return None;
        }
        let mut done = 0usize;
        let target = min(src.len(), MAX_FILE_BYTES - offset);
        while done < target {
            let cur = offset + done;
            let block_ref = cur / FS_BLOCK_SIZE;
            let block_off = cur % FS_BLOCK_SIZE;
            let n = min(target - done, FS_BLOCK_SIZE - block_off);
            let block = ensure_file_block(node, block_ref)?;
            FILE_BLOCKS[block][block_off..block_off + n].copy_from_slice(&src[done..done + n]);
            done += n;
        }
        FS_NODES[node].size = FS_NODES[node].size.max(offset + done);
        Some(done)
    }
}

unsafe fn fs_read_mutable_file(
    child: &Child,
    node: usize,
    offset: usize,
    dst: u64,
    len: usize,
) -> bool {
    unsafe {
        let mut done = 0usize;
        while done < len {
            let cur = offset + done;
            let block_ref = cur / FS_BLOCK_SIZE;
            let block_off = cur % FS_BLOCK_SIZE;
            let n = min(len - done, FS_BLOCK_SIZE - block_off);
            if block_ref >= MAX_FILE_BLOCK_REFS {
                return false;
            }
            let block = FS_NODES[node].blocks[block_ref] as usize;
            if block >= MAX_FILE_BLOCKS {
                return false;
            }
            if !copy_to_child(
                child,
                dst + done as u64,
                &FILE_BLOCKS[block][block_off..block_off + n],
            ) {
                return false;
            }
            done += n;
        }
        true
    }
}

unsafe fn ensure_file_block(node: usize, block_ref: usize) -> Option<usize> {
    unsafe {
        if block_ref >= MAX_FILE_BLOCK_REFS {
            return None;
        }
        let existing = FS_NODES[node].blocks[block_ref] as usize;
        if existing < MAX_FILE_BLOCKS {
            return Some(existing);
        }
        let block = alloc_file_block()?;
        FS_NODES[node].blocks[block_ref] = block as u16;
        Some(block)
    }
}

unsafe fn alloc_file_block() -> Option<usize> {
    unsafe {
        let mut i = 0usize;
        while i < MAX_FILE_BLOCKS {
            if !FILE_BLOCK_USED[i] {
                FILE_BLOCK_USED[i] = true;
                FILE_BLOCKS[i] = [0; FS_BLOCK_SIZE];
                return Some(i);
            }
            i += 1;
        }
        None
    }
}

fn fs_read_dir(child: &mut Child, fd: usize, dst: u64, len: usize) -> i64 {
    let node = child.fds[fd].aux;
    let mut done = 0usize;
    while done < len {
        let Some(base_offset) = fd_offset(child.fds[fd]) else {
            return -1;
        };
        let off = base_offset + done;
        let mut ent = [0u8; 16];
        if !dirent_at(node, off / 16, &mut ent) {
            break;
        }
        let start = off % 16;
        let n = min(16 - start, len - done);
        if !copy_to_child(child, dst + done as u64, &ent[start..start + n]) {
            return -1;
        }
        done += n;
    }
    advance_fd_offset(child.fds[fd], done);
    done as i64
}

fn dir_entry_count(parent: usize) -> usize {
    let mut count = 2;
    unsafe {
        let mut i = 0;
        while i < MAX_DIR_ENTRIES {
            if DIR_ENTRIES[i].used && DIR_ENTRIES[i].parent == parent {
                count += 1;
            }
            i += 1;
        }
    }
    count
}

fn dirent_at(parent: usize, idx: usize, out: &mut [u8; 16]) -> bool {
    if idx == 0 {
        return write_dirent(out, parent, b".");
    }
    if idx == 1 {
        let p = unsafe { FS_NODES[parent].parent };
        return write_dirent(out, p, b"..");
    }
    let mut seen = 2;
    unsafe {
        let mut i = 0;
        while i < MAX_DIR_ENTRIES {
            let ent = DIR_ENTRIES[i];
            if ent.used && ent.parent == parent {
                if seen == idx {
                    return write_dirent(out, ent.node, &ent.name[..ent.name_len as usize]);
                }
                seen += 1;
            }
            i += 1;
        }
    }
    false
}

fn write_dirent(out: &mut [u8; 16], node: usize, name: &[u8]) -> bool {
    unsafe {
        if node >= MAX_FS_NODES || !FS_NODES[node].used {
            return false;
        }
        for b in out.iter_mut() {
            *b = 0;
        }
        write_u16(out, 0, FS_NODES[node].ino as u16);
        let n = min(DIRSIZ, name.len());
        out[2..2 + n].copy_from_slice(&name[..n]);
        true
    }
}

fn lookup_path(child: &Child, path: &[u8]) -> Option<usize> {
    if path.is_empty() || path == b"." {
        return Some(child.cwd);
    }
    let mut cur = if path[0] == b'/' {
        FS_ROOT_NODE
    } else {
        child.cwd
    };
    let mut pos = 0usize;
    while pos < path.len() {
        while pos < path.len() && path[pos] == b'/' {
            pos += 1;
        }
        if pos >= path.len() {
            break;
        }
        let start = pos;
        while pos < path.len() && path[pos] != b'/' {
            pos += 1;
        }
        let comp = &path[start..pos];
        if comp == b"." {
            continue;
        }
        if comp == b".." {
            cur = unsafe { FS_NODES[cur].parent };
            continue;
        }
        let ent = find_dir_entry(cur, comp)?;
        cur = unsafe { DIR_ENTRIES[ent].node };
    }
    Some(cur)
}

fn lookup_parent(child: &Child, path: &[u8]) -> Option<(usize, [u8; DIRSIZ], usize)> {
    let mut end = path.len();
    while end > 0 && path[end - 1] == b'/' {
        end -= 1;
    }
    if end == 0 {
        return None;
    }
    let mut slash = end;
    while slash > 0 && path[slash - 1] != b'/' {
        slash -= 1;
    }
    let name = &path[slash..end];
    if name.is_empty() || name == b"." || name == b".." {
        return None;
    }
    let parent = if slash == 0 {
        if path[0] == b'/' {
            FS_ROOT_NODE
        } else {
            child.cwd
        }
    } else {
        lookup_path(child, &path[..slash])?
    };
    unsafe {
        if FS_NODES[parent].kind != FS_DIR {
            return None;
        }
    }
    let mut out = [0u8; DIRSIZ];
    let name_len = dir_name_len(name);
    out[..name_len].copy_from_slice(&name[..name_len]);
    Some((parent, out, name_len))
}

fn create_fs_node(parent: usize, name: &[u8], kind: u8) -> Option<usize> {
    if find_dir_entry(parent, name).is_some() {
        return None;
    }
    unsafe {
        let mut node = 0usize;
        while node < MAX_FS_NODES {
            if !FS_NODES[node].used {
                FS_NODES[node] = FsNode {
                    used: true,
                    kind,
                    ino: NEXT_INO,
                    parent,
                    nlink: 1,
                    open_refs: 0,
                    size: 0,
                    exec_index: 0,
                    blocks: [NO_FILE_BLOCK; MAX_FILE_BLOCK_REFS],
                };
                NEXT_INO = NEXT_INO.wrapping_add(1);
                add_dir_entry(parent, name, node)?;
                return Some(node);
            }
            node += 1;
        }
    }
    None
}

fn add_dir_entry(parent: usize, name: &[u8], node: usize) -> Option<usize> {
    if name.is_empty() || find_dir_entry(parent, name).is_some() {
        return None;
    }
    unsafe {
        let mut i = 0;
        while i < MAX_DIR_ENTRIES {
            if !DIR_ENTRIES[i].used {
                let mut stored = [0u8; DIRSIZ];
                let name_len = dir_name_len(name);
                stored[..name_len].copy_from_slice(&name[..name_len]);
                DIR_ENTRIES[i] = DirEntry {
                    used: true,
                    parent,
                    node,
                    name_len: name_len as u8,
                    name: stored,
                };
                return Some(i);
            }
            i += 1;
        }
    }
    None
}

fn find_dir_entry(parent: usize, name: &[u8]) -> Option<usize> {
    if name.is_empty() {
        return None;
    }
    let name_len = dir_name_len(name);
    unsafe {
        let mut i = 0;
        while i < MAX_DIR_ENTRIES {
            let ent = DIR_ENTRIES[i];
            if ent.used
                && ent.parent == parent
                && ent.name_len as usize == name_len
                && &ent.name[..name_len] == &name[..name_len]
            {
                return Some(i);
            }
            i += 1;
        }
    }
    None
}

fn dir_name_len(name: &[u8]) -> usize {
    min(DIRSIZ, name.len())
}

fn sys_unlink(child: &Child, path_ptr: u64) -> i64 {
    let mut path = [0u8; 128];
    let Some(len) = copy_cstr_from_child(child, path_ptr, &mut path) else {
        return -1;
    };
    let Some((parent, name, name_len)) = lookup_parent(child, &path[..len]) else {
        return -1;
    };
    let Some(ent_idx) = find_dir_entry(parent, &name[..name_len]) else {
        return -1;
    };
    unsafe {
        let node = DIR_ENTRIES[ent_idx].node;
        if node == FS_README_NODE || node == FS_CONSOLE_NODE {
            return -1;
        }
        if FS_NODES[node].kind == FS_DIR && !is_dir_empty(node) {
            return -1;
        }
        DIR_ENTRIES[ent_idx] = DirEntry::empty();
        if FS_NODES[node].nlink > 0 {
            FS_NODES[node].nlink -= 1;
        }
        maybe_free_node(node);
    }
    0
}

fn sys_link(child: &Child, old_ptr: u64, new_ptr: u64) -> i64 {
    let mut old_path = [0u8; 128];
    let mut new_path = [0u8; 128];
    let Some(old_len) = copy_cstr_from_child(child, old_ptr, &mut old_path) else {
        return -1;
    };
    let Some(new_len) = copy_cstr_from_child(child, new_ptr, &mut new_path) else {
        return -1;
    };
    let Some(node) = lookup_path(child, &old_path[..old_len]) else {
        return -1;
    };
    let Some((parent, name, name_len)) = lookup_parent(child, &new_path[..new_len]) else {
        return -1;
    };
    unsafe {
        if FS_NODES[node].kind != FS_FILE && FS_NODES[node].kind != FS_README {
            return -1;
        }
        if add_dir_entry(parent, &name[..name_len], node).is_none() {
            return -1;
        }
        FS_NODES[node].nlink = FS_NODES[node].nlink.saturating_add(1);
    }
    0
}

fn sys_mkdir(child: &Child, path_ptr: u64) -> i64 {
    let mut path = [0u8; 128];
    let Some(len) = copy_cstr_from_child(child, path_ptr, &mut path) else {
        return -1;
    };
    let Some((parent, name, name_len)) = lookup_parent(child, &path[..len]) else {
        return -1;
    };
    if create_fs_node(parent, &name[..name_len], FS_DIR).is_some() {
        0
    } else {
        -1
    }
}

fn is_dir_empty(node: usize) -> bool {
    unsafe {
        let mut i = 0;
        while i < MAX_DIR_ENTRIES {
            if DIR_ENTRIES[i].used && DIR_ENTRIES[i].parent == node {
                return false;
            }
            i += 1;
        }
    }
    true
}

fn maybe_free_node(node: usize) {
    unsafe {
        if node >= MAX_FS_NODES || !FS_NODES[node].used {
            return;
        }
        if FS_NODES[node].nlink == 0 && FS_NODES[node].open_refs == 0 {
            truncate_file(node);
            FS_NODES[node] = FsNode::empty();
        }
    }
}

unsafe fn truncate_file(node: usize) {
    unsafe {
        if node >= MAX_FS_NODES || FS_NODES[node].kind != FS_FILE {
            if node < MAX_FS_NODES {
                FS_NODES[node].size = 0;
            }
            return;
        }
        let mut i = 0usize;
        while i < MAX_FILE_BLOCK_REFS {
            let block = FS_NODES[node].blocks[i] as usize;
            if block < MAX_FILE_BLOCKS {
                FILE_BLOCK_USED[block] = false;
                FILE_BLOCKS[block] = [0; FS_BLOCK_SIZE];
                FS_NODES[node].blocks[i] = NO_FILE_BLOCK;
            }
            i += 1;
        }
        FS_NODES[node].size = 0;
    }
}

unsafe fn pipe_write(pipe_idx: usize, src: &[u8]) -> usize {
    unsafe {
        if pipe_idx >= MAX_PIPES || PIPES[pipe_idx].readers == 0 {
            return 0;
        }
        let pipe = &mut PIPES[pipe_idx];
        let mut n = 0;
        while n < src.len() && pipe.len < PIPE_BUF {
            let write_pos = (pipe.read_pos + pipe.len) % PIPE_BUF;
            pipe.buf[write_pos] = src[n];
            pipe.len += 1;
            n += 1;
        }
        n
    }
}

unsafe fn pipe_has_readers(pipe_idx: usize) -> bool {
    unsafe { pipe_idx < MAX_PIPES && PIPES[pipe_idx].readers > 0 }
}

unsafe fn pipe_read(child: &Child, pipe_idx: usize, dst: u64, len: usize) -> i64 {
    unsafe {
        if pipe_idx >= MAX_PIPES {
            return -1;
        }
        let pipe = &mut PIPES[pipe_idx];
        if pipe.len == 0 {
            return 0;
        }

        let mut total = 0usize;
        let mut scratch = [0u8; 128];
        while total < len && pipe.len > 0 {
            let n = min(scratch.len(), min(len - total, pipe.len));
            let mut i = 0;
            while i < n {
                scratch[i] = pipe.buf[pipe.read_pos];
                pipe.read_pos = (pipe.read_pos + 1) % PIPE_BUF;
                pipe.len -= 1;
                i += 1;
            }
            if !copy_to_child(child, dst + total as u64, &scratch[..n]) {
                return -1;
            }
            total += n;
        }
        total as i64
    }
}

fn basename(path: &[u8]) -> &[u8] {
    let mut start = 0;
    for (i, b) in path.iter().enumerate() {
        if *b == b'/' {
            start = i + 1;
        }
    }
    &path[start..]
}
