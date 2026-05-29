use core::cmp::min;

use crate::allocator::Allocator;
use crate::child::{copy_cstr_from_child, copy_from_child, copy_to_child, map_fresh_child_page};
use crate::consts::*;
use crate::sel4::sel4_yield;
use crate::types::{Child, FdEntry};
use crate::util::*;

static mut FD_TABLE: [FdEntry; MAX_FD] = [FdEntry::closed(); MAX_FD];
static mut TICKS: u64 = 0;

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

pub(crate) fn handle_xv6_syscall(alloc: &mut Allocator, child: &mut Child, mrs: &[u64; 16]) -> i64 {
    let sysno = mrs[10];
    let a0 = mrs[3];
    let a1 = mrs[4];
    let a2 = mrs[5];
    tick();

    match sysno {
        SYS_EXIT => {
            log("xv6-host: exit(");
            print_i64(a0 as i64);
            log(")\n");
            halt_loop();
        }
        SYS_WRITE => sys_write(a0 as usize, a1, a2 as usize),
        SYS_READ => sys_read(a0 as usize, a1, a2 as usize),
        SYS_OPEN => sys_open(a0),
        SYS_CLOSE => sys_close(a0 as usize),
        SYS_DUP => sys_dup(a0 as usize),
        SYS_FSTAT => sys_fstat(a0 as usize, a1),
        SYS_SBRK => sys_sbrk(alloc, child, a0 as i64),
        SYS_GETPID => 1,
        SYS_UPTIME => unsafe { TICKS as i64 },
        SYS_PAUSE => {
            unsafe { sel4_yield() };
            0
        }
        SYS_FORK | SYS_WAIT => -1,
        _ => -1,
    }
}

fn sys_write(fd: usize, buf: u64, len: usize) -> i64 {
    if len == 0 {
        return 0;
    }
    if fd >= MAX_FD || unsafe { FD_TABLE[fd].kind } != FD_CONSOLE || fd == 0 {
        return -1;
    }
    let mut scratch = [0u8; 128];
    let mut done = 0usize;
    while done < len {
        let n = min(scratch.len(), len - done);
        if !copy_from_child(buf + done as u64, &mut scratch[..n]) {
            return -1;
        }
        for b in &scratch[..n] {
            putchar(*b);
        }
        done += n;
    }
    len as i64
}

fn sys_read(fd: usize, dst: u64, len: usize) -> i64 {
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
                if !copy_to_child(dst, &data[entry.offset..entry.offset + n]) {
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
                if !copy_to_child(dst, &data[entry.offset..entry.offset + n]) {
                    return -1;
                }
                FD_TABLE[fd].offset += n;
                n as i64
            }
            _ => -1,
        }
    }
}

fn sys_open(path_ptr: u64) -> i64 {
    let mut path = [0u8; 128];
    let Some(len) = copy_cstr_from_child(path_ptr, &mut path) else {
        return -1;
    };
    let name = basename(&path[..len]);
    let kind = if path_is_root(&path[..len]) || name == b"." || name == b".." {
        FD_ROOTDIR
    } else if name == b"README" {
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
        FD_TABLE[fd] = FdEntry::closed();
    }
    0
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
                return i as i64;
            }
        }
    }
    -1
}

fn sys_fstat(fd: usize, dst: u64) -> i64 {
    if fd >= MAX_FD {
        return -1;
    }
    let entry = unsafe { FD_TABLE[fd] };
    let (typ, ino, size) = match entry.kind {
        FD_CONSOLE => (T_DEVICE, CONSOLE_INO, 0u64),
        FD_README => (T_FILE, README_INO, README_BYTES.len() as u64),
        FD_ROOTDIR => (T_DIR, ROOT_INO, ROOT_DIRENTS.len() as u64),
        _ => return -1,
    };
    let mut st = [0u8; 24];
    write_i32(&mut st, 0, 1);
    write_u32(&mut st, 4, ino);
    write_u16(&mut st, 8, typ);
    write_u16(&mut st, 10, 1);
    write_u64_bytes(&mut st, 16, size);
    if !copy_to_child(dst, &st) {
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
            map_fresh_child_page(alloc, child.vspace, page, true, false);
            page += PAGE_SIZE;
        }
        child.heap_mapped_end = align_up(new_brk);
    }
    child.brk = new_brk;
    old as i64
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
        };
        FD_TABLE[1] = FdEntry {
            kind: FD_CONSOLE,
            offset: 0,
        };
        FD_TABLE[2] = FdEntry {
            kind: FD_CONSOLE,
            offset: 0,
        };
    }
}

fn alloc_fd(kind: u8) -> i64 {
    unsafe {
        for i in 0..MAX_FD {
            if FD_TABLE[i].kind == FD_CLOSED {
                FD_TABLE[i] = FdEntry { kind, offset: 0 };
                return i as i64;
            }
        }
    }
    -1
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
