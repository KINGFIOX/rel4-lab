use core::cmp::min;

use crate::allocator::Allocator;
use crate::child::{
    clear_process_mappings, clone_address_space, copy_cstr_from_child, copy_from_child,
    copy_to_child, create_child, load_elf, map_fresh_child_page, map_stack, read_user_context,
    reset_process_mappings, write_user_context,
};
use crate::consts::*;
use crate::sel4::{call_checked, msg_info, sel4_send, sel4_yield};
use crate::types::{Child, FdEntry};
use crate::util::*;

static mut FD_TABLE: [FdEntry; MAX_FD] = [FdEntry::closed(); MAX_FD];
static mut TICKS: u64 = 0;
static mut PIPES: [Pipe; MAX_PIPES] = [Pipe::closed(); MAX_PIPES];
static mut NEXT_PID: u64 = 2;

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

const ROOT_DIRENTS: [u8; 64] = [
    1, 0, b'.', 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 1, 0, b'.', b'.', 0, 0, 0, 0, 0, 0, 0, 0, 0,
    0, 0, 0, 2, 0, b'R', b'E', b'A', b'D', b'M', b'E', 0, 0, 0, 0, 0, 0, 0, 0, 3, 0, b'c', b'o',
    b'n', b's', b'o', b'l', b'e', 0, 0, 0, 0, 0, 0, 0,
];

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
            return sys_exit(procs, proc_idx, a0 as i32);
        }
        SYS_WAIT => return sys_wait(alloc, procs, proc_idx, a0, mrs),
        SYS_WRITE => sys_write(&procs[proc_idx], a0 as usize, a1, a2 as usize),
        SYS_READ => sys_read(&procs[proc_idx], a0 as usize, a1, a2 as usize),
        SYS_OPEN => sys_open(&procs[proc_idx], a0, a1 as u32),
        SYS_CLOSE => sys_close(a0 as usize),
        SYS_DUP => sys_dup(a0 as usize),
        SYS_FSTAT => sys_fstat(&procs[proc_idx], a0 as usize, a1),
        SYS_SBRK => sys_sbrk(alloc, &mut procs[proc_idx], a0 as i64),
        SYS_GETPID => procs[proc_idx].pid as i64,
        SYS_UPTIME => unsafe { TICKS as i64 },
        SYS_PAUSE => {
            unsafe { sel4_yield() };
            0
        }
        SYS_KILL => sys_kill(procs, a0 as i64),
        SYS_CHDIR => sys_chdir(&procs[proc_idx], a0),
        SYS_PIPE => sys_pipe(&procs[proc_idx], a0),
        SYS_MKNOD => sys_mknod(&procs[proc_idx], a0),
        SYS_EXEC => return sys_exec(alloc, &mut procs[proc_idx], a0, a1),
        SYS_UNLINK | SYS_LINK | SYS_MKDIR => -1,
        _ => -1,
    };
    SyscallResult::Reply(ret)
}

fn sys_write(child: &Child, fd: usize, buf: u64, len: usize) -> i64 {
    if len == 0 {
        return 0;
    }
    let mut scratch = [0u8; 128];
    let mut done = 0usize;
    while done < len {
        let n = min(scratch.len(), len - done);
        if !copy_from_child(child, buf + done as u64, &mut scratch[..n]) {
            return -1;
        }
        let wrote = unsafe {
            if fd >= MAX_FD {
                return -1;
            }
            match FD_TABLE[fd].kind {
                FD_CONSOLE if fd != 0 => {
                    for b in &scratch[..n] {
                        putchar(*b);
                    }
                    n
                }
                FD_PIPE_WRITE => pipe_write(FD_TABLE[fd].aux, &scratch[..n]),
                _ => return -1,
            }
        };
        if wrote == 0 {
            break;
        }
        done += wrote;
    }
    done as i64
}

fn sys_read(child: &Child, fd: usize, dst: u64, len: usize) -> i64 {
    if fd >= MAX_FD || len == 0 {
        return if fd < MAX_FD { 0 } else { -1 };
    }
    unsafe {
        let entry = FD_TABLE[fd];
        match entry.kind {
            FD_CONSOLE => 0,
            FD_README => {
                let data = README_BYTES;
                if entry.offset >= data.len() {
                    return 0;
                }
                let n = min(len, data.len() - entry.offset);
                if !copy_to_child(child, dst, &data[entry.offset..entry.offset + n]) {
                    return -1;
                }
                FD_TABLE[fd].offset += n;
                n as i64
            }
            FD_ROOTDIR => {
                let data = &ROOT_DIRENTS;
                if entry.offset >= data.len() {
                    return 0;
                }
                let n = min(len, data.len() - entry.offset);
                if !copy_to_child(child, dst, &data[entry.offset..entry.offset + n]) {
                    return -1;
                }
                FD_TABLE[fd].offset += n;
                n as i64
            }
            FD_PIPE_READ => pipe_read(child, entry.aux, dst, len),
            _ => -1,
        }
    }
}

fn sys_open(child: &Child, path_ptr: u64, flags: u32) -> i64 {
    let mut path = [0u8; 128];
    let Some(len) = copy_cstr_from_child(child, path_ptr, &mut path) else {
        return -1;
    };
    let name = basename(&path[..len]);
    let wants_write = flags & (O_WRONLY | O_RDWR | O_CREATE | O_TRUNC) != 0;
    let kind = if path_is_root(&path[..len]) || name == b"." || name == b".." {
        if wants_write {
            return -1;
        }
        FD_ROOTDIR
    } else if name == b"README" {
        if wants_write {
            return -1;
        }
        FD_README
    } else if name == b"console" {
        FD_CONSOLE
    } else {
        return -1;
    };
    alloc_fd(kind)
}

fn sys_close(fd: usize) -> i64 {
    if fd >= MAX_FD {
        return -1;
    }
    unsafe {
        if fd <= 2 {
            FD_TABLE[fd].offset = 0;
            return 0;
        }
        if FD_TABLE[fd].kind == FD_CLOSED {
            return -1;
        }
        close_fd(fd);
        return 0;
    }
}

unsafe fn close_fd(fd: usize) {
    unsafe {
        match FD_TABLE[fd].kind {
            FD_PIPE_READ => {
                let pipe = FD_TABLE[fd].aux;
                if pipe < MAX_PIPES && PIPES[pipe].readers > 0 {
                    PIPES[pipe].readers -= 1;
                }
            }
            FD_PIPE_WRITE => {
                let pipe = FD_TABLE[fd].aux;
                if pipe < MAX_PIPES && PIPES[pipe].writers > 0 {
                    PIPES[pipe].writers -= 1;
                }
            }
            _ => {}
        }
        FD_TABLE[fd] = FdEntry::closed();
    }
}

fn sys_dup(fd: usize) -> i64 {
    if fd >= MAX_FD {
        return -1;
    }
    let entry = unsafe { FD_TABLE[fd] };
    if entry.kind == FD_CLOSED {
        return -1;
    }
    unsafe {
        for i in 0..MAX_FD {
            if FD_TABLE[i].kind == FD_CLOSED {
                FD_TABLE[i] = entry;
                match entry.kind {
                    FD_PIPE_READ if entry.aux < MAX_PIPES => PIPES[entry.aux].readers += 1,
                    FD_PIPE_WRITE if entry.aux < MAX_PIPES => PIPES[entry.aux].writers += 1,
                    _ => {}
                }
                return i as i64;
            }
        }
    }
    -1
}

fn sys_fstat(child: &Child, fd: usize, dst: u64) -> i64 {
    if fd >= MAX_FD {
        return -1;
    }
    let entry = unsafe { FD_TABLE[fd] };
    let (typ, ino, size) = match entry.kind {
        FD_CONSOLE => (T_DEVICE, CONSOLE_INO, 0u64),
        FD_README => (T_FILE, README_INO, README_BYTES.len() as u64),
        FD_ROOTDIR => (T_DIR, ROOT_INO, ROOT_DIRENTS.len() as u64),
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

fn sys_sbrk(alloc: &mut Allocator, child: &mut Child, increment: i64) -> i64 {
    let old = child.brk;
    let new_brk = if increment >= 0 {
        old.saturating_add(increment as u64)
    } else {
        old.saturating_sub((-increment) as u64)
    };
    if new_brk > CHILD_HEAP_LIMIT {
        return -1;
    }
    if new_brk > child.heap_mapped_end {
        let mut page = align_up(child.heap_mapped_end);
        while page < align_up(new_brk) {
            map_fresh_child_page(alloc, child, page, true, false);
            page += PAGE_SIZE;
        }
        child.heap_mapped_end = align_up(new_brk);
    }
    child.brk = new_brk;
    old as i64
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

    let mut child = create_child(alloc, pid, parent.pid, parent.fault_ep);
    child.entry = parent.entry;
    child.brk = parent.brk;
    child.heap_mapped_end = parent.heap_mapped_end;
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

fn sys_exit(procs: &mut [Child; MAX_PROCS], proc_idx: usize, status: i32) -> SyscallResult {
    let pid = procs[proc_idx].pid;
    log("xv6-host: exit(");
    print_i64(status as i64);
    log(") pid=");
    print_u64(pid);
    log("\n");
    if procs[proc_idx].parent_pid == 0 {
        halt_loop();
    }
    procs[proc_idx].state = PROC_ZOMBIE;
    procs[proc_idx].exit_status = status;
    reply_waiting_parent(procs, proc_idx);
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
        clear_process_mappings(procs[i].pid);
        procs[i] = Child::empty();
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

fn reply_waiting_parent(procs: &mut [Child; MAX_PROCS], child_idx: usize) {
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
            clear_process_mappings(procs[child_idx].pid);
            procs[child_idx] = Child::empty();
        }
        return;
    }
}

fn sys_kill(procs: &mut [Child; MAX_PROCS], pid: i64) -> i64 {
    if pid <= 0 {
        return -1;
    }
    for proc in procs.iter_mut() {
        if proc.pid == pid as u64 && proc.state != PROC_UNUSED {
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

    reset_process_mappings(child.pid);
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

fn sys_chdir(child: &Child, path_ptr: u64) -> i64 {
    let mut path = [0u8; 128];
    let Some(len) = copy_cstr_from_child(child, path_ptr, &mut path) else {
        return -1;
    };
    if path_is_root(&path[..len]) || basename(&path[..len]) == b".." {
        0
    } else {
        -1
    }
}

fn sys_pipe(child: &Child, fds_ptr: u64) -> i64 {
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

        let Some(read_fd) = find_free_fd() else {
            return -1;
        };
        FD_TABLE[read_fd] = FdEntry {
            kind: FD_PIPE_READ,
            offset: 0,
            aux: pipe_idx,
        };
        let Some(write_fd) = find_free_fd() else {
            FD_TABLE[read_fd] = FdEntry::closed();
            return -1;
        };

        PIPES[pipe_idx] = Pipe::closed();
        PIPES[pipe_idx].readers = 1;
        PIPES[pipe_idx].writers = 1;
        FD_TABLE[write_fd] = FdEntry {
            kind: FD_PIPE_WRITE,
            offset: 0,
            aux: pipe_idx,
        };

        let mut out = [0u8; 8];
        write_i32(&mut out, 0, read_fd as i32);
        write_i32(&mut out, 4, write_fd as i32);
        if !copy_to_child(child, fds_ptr, &out) {
            close_fd(read_fd);
            close_fd(write_fd);
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

pub(crate) fn init_fds() {
    unsafe {
        let mut i = 0;
        while i < MAX_FD {
            FD_TABLE[i] = FdEntry::closed();
            i += 1;
        }
        FD_TABLE[0] = FdEntry {
            kind: FD_CONSOLE,
            offset: 0,
            aux: 0,
        };
        FD_TABLE[1] = FdEntry {
            kind: FD_CONSOLE,
            offset: 0,
            aux: 0,
        };
        FD_TABLE[2] = FdEntry {
            kind: FD_CONSOLE,
            offset: 0,
            aux: 0,
        };
        let mut p = 0;
        while p < MAX_PIPES {
            PIPES[p] = Pipe::closed();
            p += 1;
        }
    }
}

fn alloc_fd(kind: u8) -> i64 {
    unsafe {
        if let Some(i) = find_free_fd() {
            FD_TABLE[i] = FdEntry {
                kind,
                offset: 0,
                aux: 0,
            };
            return i as i64;
        }
    }
    -1
}

fn find_free_fd() -> Option<usize> {
    unsafe {
        for i in 0..MAX_FD {
            if FD_TABLE[i].kind == FD_CLOSED {
                return Some(i);
            }
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

fn path_is_root(path: &[u8]) -> bool {
    path.is_empty() || path == b"/" || path == b"."
}
