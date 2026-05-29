//! Minimal xv6 user-program compatibility path.
//!
//! This is deliberately small: it lets a single xv6 user ELF run as the
//! initial rootserver and handles enough of xv6's Unix-like syscall ABI for
//! simple programs (`echo`, printf-heavy smoke tests, basic heap users) to
//! execute. Process creation, exec, pipes, and a real filesystem belong in a
//! later user-space compatibility server layer.

use core::cell::UnsafeCell;
use core::sync::atomic::{AtomicBool, AtomicU64, Ordering};

use crate::arch::riscv64::sv39::PAGE_SIZE;
use crate::arch::riscv64::trap::{UserContext, reg};
use crate::arch::riscv64::{csr, sbi};

const SYS_FORK: isize = 1;
const SYS_EXIT: isize = 2;
const SYS_WAIT: isize = 3;
const SYS_PIPE: isize = 4;
const SYS_READ: isize = 5;
const SYS_KILL: isize = 6;
const SYS_EXEC: isize = 7;
const SYS_FSTAT: isize = 8;
const SYS_CHDIR: isize = 9;
const SYS_DUP: isize = 10;
const SYS_GETPID: isize = 11;
const SYS_SBRK: isize = 12;
const SYS_PAUSE: isize = 13;
const SYS_UPTIME: isize = 14;
const SYS_OPEN: isize = 15;
const SYS_WRITE: isize = 16;
const SYS_MKNOD: isize = 17;
const SYS_UNLINK: isize = 18;
const SYS_LINK: isize = 19;
const SYS_MKDIR: isize = 20;
const SYS_CLOSE: isize = 21;

const MAX_FD: usize = 32;
const STDIN_FD: usize = 0;
const STDERR_FD: usize = 2;
const XV6_ROOTSERVER_BASE: usize = 0x1000_0000;
const MAX_PATH: usize = 128;

const T_DIR: i16 = 1;
const T_FILE: i16 = 2;
const T_DEVICE: i16 = 3;
const ROOT_INO: u32 = 1;
const README_INO: u32 = 2;
const CONSOLE_INO: u32 = 3;

static README_BYTES: &[u8] = include_bytes!("../../third_party/xv6-riscv/README");

static ENABLED: AtomicBool = AtomicBool::new(false);
static ROOT_PT: AtomicU64 = AtomicU64::new(0);
static BRK: AtomicU64 = AtomicU64::new(0);
static HEAP_MAPPED_END: AtomicU64 = AtomicU64::new(0);
static FD_BITMAP: AtomicU64 = AtomicU64::new(0b111);

#[repr(u8)]
#[derive(Copy, Clone, PartialEq, Eq)]
enum FdKind {
    Closed = 0,
    Console = 1,
    Readme = 2,
    RootDir = 3,
}

#[derive(Copy, Clone)]
struct FdEntry {
    kind: FdKind,
    offset: usize,
}

impl FdEntry {
    const fn closed() -> Self {
        Self {
            kind: FdKind::Closed,
            offset: 0,
        }
    }

    const fn console() -> Self {
        Self {
            kind: FdKind::Console,
            offset: 0,
        }
    }
}

#[repr(transparent)]
struct FdTable(UnsafeCell<[FdEntry; MAX_FD]>);
unsafe impl Sync for FdTable {}

static FD_TABLE: FdTable = FdTable(UnsafeCell::new([const { FdEntry::closed() }; MAX_FD]));

#[repr(C)]
#[derive(Copy, Clone)]
struct Xv6Stat {
    dev: i32,
    ino: u32,
    type_: i16,
    nlink: i16,
    size: u64,
}

#[repr(C)]
#[derive(Copy, Clone)]
struct Xv6Dirent {
    inum: u16,
    name: [u8; 14],
}

static ROOT_DIRENTS: [Xv6Dirent; 4] = [
    Xv6Dirent {
        inum: ROOT_INO as u16,
        name: [b'.', 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0],
    },
    Xv6Dirent {
        inum: ROOT_INO as u16,
        name: [b'.', b'.', 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0],
    },
    Xv6Dirent {
        inum: README_INO as u16,
        name: [b'R', b'E', b'A', b'D', b'M', b'E', 0, 0, 0, 0, 0, 0, 0, 0],
    },
    Xv6Dirent {
        inum: CONSOLE_INO as u16,
        name: [
            b'c', b'o', b'n', b's', b'o', b'l', b'e', 0, 0, 0, 0, 0, 0, 0,
        ],
    },
];

#[inline]
pub fn looks_like_xv6_rootserver(user_va_start: usize, entry: usize) -> bool {
    (user_va_start == 0 && entry < 0x10_0000)
        || (user_va_start == XV6_ROOTSERVER_BASE
            && entry >= XV6_ROOTSERVER_BASE
            && entry < XV6_ROOTSERVER_BASE + 0x10_0000)
}

pub fn init(root_pt: *mut crate::arch::riscv64::sv39::PageTable, user_va_end: usize) {
    let heap_start = align_up(user_va_end as u64, PAGE_SIZE as u64);
    ROOT_PT.store(root_pt as u64, Ordering::Release);
    BRK.store(heap_start, Ordering::Release);
    HEAP_MAPPED_END.store(heap_start, Ordering::Release);
    reset_fds();
    ENABLED.store(true, Ordering::Release);
    crate::println!(
        "  xv6 compat: enabled heap_start={:#x} root_pt={:#x}",
        heap_start,
        root_pt as usize,
    );
}

#[inline]
pub fn is_xv6_syscall(n: isize) -> bool {
    ENABLED.load(Ordering::Acquire) && (SYS_FORK..=SYS_CLOSE).contains(&n)
}

pub fn handle_syscall(uc: &mut UserContext, sysno: isize) {
    match sysno {
        SYS_EXIT => exit(uc.regs[reg::A0] as i64),
        SYS_WRITE => {
            let fd = uc.regs[reg::A0] as usize;
            let buf = uc.regs[reg::A1] as *const u8;
            let len = uc.regs[reg::A2] as usize;
            ret(uc, write(fd, buf, len));
        }
        SYS_READ => {
            let fd = uc.regs[reg::A0] as usize;
            let buf = uc.regs[reg::A1] as *mut u8;
            let len = uc.regs[reg::A2] as usize;
            ret(uc, read(fd, buf, len));
        }
        SYS_SBRK => {
            let increment = uc.regs[reg::A0] as i64;
            ret(uc, sbrk(increment));
        }
        SYS_GETPID => ret(uc, 1),
        SYS_UPTIME => ret(uc, (csr::time() as u64 / 10_000) as i64),
        SYS_OPEN => {
            let path = uc.regs[reg::A0] as *const u8;
            ret(uc, open(path));
        }
        SYS_CLOSE => {
            let fd = uc.regs[reg::A0] as usize;
            ret(uc, close(fd));
        }
        SYS_DUP => {
            let fd = uc.regs[reg::A0] as usize;
            ret(uc, dup(fd));
        }
        SYS_PAUSE => ret(uc, 0),
        SYS_FSTAT => {
            let fd = uc.regs[reg::A0] as usize;
            let st = uc.regs[reg::A1] as *mut Xv6Stat;
            ret(uc, fstat(fd, st));
        }
        SYS_FORK | SYS_WAIT | SYS_PIPE | SYS_KILL | SYS_EXEC | SYS_CHDIR | SYS_MKNOD
        | SYS_UNLINK | SYS_LINK | SYS_MKDIR => {
            ret(uc, -1);
        }
        _ => ret(uc, -1),
    }
}

fn exit(status: i64) -> ! {
    crate::println!("xv6compat: exit({})", status);
    sbi::shutdown()
}

fn write(fd: usize, buf: *const u8, len: usize) -> i64 {
    if len == 0 {
        return 0;
    }
    if fd_kind(fd) != Some(FdKind::Console) || fd == STDIN_FD || buf.is_null() {
        return -1;
    }
    let max = len.min(1 << 20);
    for i in 0..max {
        let ch = unsafe { core::ptr::read_volatile(buf.add(i)) };
        sbi::console_putchar(ch);
    }
    max as i64
}

fn read(fd: usize, buf: *mut u8, len: usize) -> i64 {
    if len == 0 {
        return 0;
    }
    if buf.is_null() {
        return -1;
    }
    match fd_kind(fd) {
        Some(FdKind::Console) if fd == STDIN_FD => {
            let ch = sbi::console_getchar();
            if ch < 0 {
                return 0;
            }
            unsafe {
                core::ptr::write_volatile(buf, ch as u8);
            }
            1
        }
        Some(FdKind::Readme) => read_bytes(fd, README_BYTES, buf, len),
        Some(FdKind::RootDir) => read_bytes(fd, root_dir_bytes(), buf, len),
        _ => -1,
    }
}

fn sbrk(increment: i64) -> i64 {
    let old = BRK.load(Ordering::Acquire);
    let Some(new) = add_signed(old, increment) else {
        return -1;
    };
    if new
        > (crate::kernel::boot::USER_STACK_TOP - crate::kernel::boot::USER_STACK_PAGES * PAGE_SIZE)
            as u64
    {
        return -1;
    }
    if new > HEAP_MAPPED_END.load(Ordering::Acquire) && !map_heap_to(new) {
        return -1;
    }
    BRK.store(new, Ordering::Release);
    old as i64
}

fn map_heap_to(new_brk: u64) -> bool {
    let root = ROOT_PT.load(Ordering::Acquire) as *mut crate::arch::riscv64::sv39::PageTable;
    if root.is_null() {
        return false;
    }
    let mut mapped_end = HEAP_MAPPED_END.load(Ordering::Acquire);
    let target = align_up(new_brk, PAGE_SIZE as u64);
    while mapped_end < target {
        let kva = crate::kernel::bootmem::alloc_page();
        let pa = crate::arch::riscv64::vspace::kpptr_to_paddr(kva);
        unsafe {
            crate::arch::riscv64::vspace::map_user_4k(
                root,
                mapped_end as usize,
                pa,
                crate::arch::riscv64::vspace::user_flags(true, true, false),
            );
        }
        mapped_end += PAGE_SIZE as u64;
    }
    HEAP_MAPPED_END.store(mapped_end, Ordering::Release);
    true
}

fn open(path: *const u8) -> i64 {
    match path_kind(path) {
        Some(FdKind::Console) => alloc_fd(FdEntry::console()),
        Some(FdKind::Readme) => alloc_fd(FdEntry {
            kind: FdKind::Readme,
            offset: 0,
        }),
        Some(FdKind::RootDir) => alloc_fd(FdEntry {
            kind: FdKind::RootDir,
            offset: 0,
        }),
        _ => -1,
    }
}

fn close(fd: usize) -> i64 {
    if fd >= MAX_FD || fd_kind(fd).is_none() {
        return -1;
    }
    unsafe {
        (*FD_TABLE.0.get())[fd] = FdEntry::closed();
    }
    let mask = !(1u64 << fd);
    FD_BITMAP.fetch_and(mask, Ordering::AcqRel);
    0
}

fn dup(fd: usize) -> i64 {
    let Some(entry) = fd_entry(fd) else {
        return -1;
    };
    alloc_fd(entry)
}

fn fstat(fd: usize, st: *mut Xv6Stat) -> i64 {
    if st.is_null() {
        return -1;
    }
    let Some(kind) = fd_kind(fd) else {
        return -1;
    };
    let stat = match kind {
        FdKind::Console => Xv6Stat {
            dev: 1,
            ino: CONSOLE_INO,
            type_: T_DEVICE,
            nlink: 1,
            size: 0,
        },
        FdKind::Readme => Xv6Stat {
            dev: 1,
            ino: README_INO,
            type_: T_FILE,
            nlink: 1,
            size: README_BYTES.len() as u64,
        },
        FdKind::RootDir => Xv6Stat {
            dev: 1,
            ino: ROOT_INO,
            type_: T_DIR,
            nlink: 1,
            size: root_dir_bytes().len() as u64,
        },
        FdKind::Closed => return -1,
    };
    unsafe {
        core::ptr::write_volatile(st, stat);
    }
    0
}

fn alloc_fd(entry: FdEntry) -> i64 {
    loop {
        let cur = FD_BITMAP.load(Ordering::Acquire);
        for fd in 0..MAX_FD {
            let bit = 1u64 << fd;
            if cur & bit == 0 {
                if FD_BITMAP
                    .compare_exchange(cur, cur | bit, Ordering::AcqRel, Ordering::Acquire)
                    .is_ok()
                {
                    unsafe {
                        (*FD_TABLE.0.get())[fd] = entry;
                    }
                    return fd as i64;
                }
                break;
            }
        }
        if cur.count_ones() as usize >= MAX_FD {
            return -1;
        }
    }
}

#[inline]
fn fd_kind(fd: usize) -> Option<FdKind> {
    fd_entry(fd).map(|entry| entry.kind)
}

fn fd_entry(fd: usize) -> Option<FdEntry> {
    if fd >= MAX_FD || (FD_BITMAP.load(Ordering::Acquire) & (1u64 << fd)) == 0 {
        return None;
    }
    let entry = unsafe { (*FD_TABLE.0.get())[fd] };
    if entry.kind == FdKind::Closed {
        None
    } else {
        Some(entry)
    }
}

fn reset_fds() {
    unsafe {
        let fds = &mut *FD_TABLE.0.get();
        for entry in fds.iter_mut() {
            *entry = FdEntry::closed();
        }
        fds[0] = FdEntry::console();
        fds[1] = FdEntry::console();
        fds[2] = FdEntry::console();
    }
    FD_BITMAP.store(0b111, Ordering::Release);
}

fn read_bytes(fd: usize, source: &[u8], dst: *mut u8, len: usize) -> i64 {
    if fd >= MAX_FD {
        return -1;
    }
    let mut entry = unsafe { (*FD_TABLE.0.get())[fd] };
    if entry.offset >= source.len() {
        return 0;
    }
    let n = len.min(source.len() - entry.offset);
    for i in 0..n {
        unsafe {
            core::ptr::write_volatile(dst.add(i), source[entry.offset + i]);
        }
    }
    entry.offset += n;
    unsafe {
        (*FD_TABLE.0.get())[fd] = entry;
    }
    n as i64
}

fn root_dir_bytes() -> &'static [u8] {
    unsafe {
        core::slice::from_raw_parts(
            ROOT_DIRENTS.as_ptr() as *const u8,
            core::mem::size_of_val(&ROOT_DIRENTS),
        )
    }
}

fn path_kind(ptr: *const u8) -> Option<FdKind> {
    let mut buf = [0u8; MAX_PATH];
    let len = read_user_cstr(ptr, &mut buf)?;
    let mut path = &buf[..len];
    while path.starts_with(b"./") {
        path = &path[2..];
    }
    if matches!(path, b"console" | b"/console") {
        Some(FdKind::Console)
    } else if matches!(path, b"README" | b"/README") {
        Some(FdKind::Readme)
    } else if matches!(path, b"." | b".." | b"/" | b"") {
        Some(FdKind::RootDir)
    } else {
        None
    }
}

fn read_user_cstr(ptr: *const u8, out: &mut [u8]) -> Option<usize> {
    if ptr.is_null() {
        return None;
    }
    for i in 0..out.len() {
        let ch = unsafe { core::ptr::read_volatile(ptr.add(i)) };
        if ch == 0 {
            return Some(i);
        }
        out[i] = ch;
    }
    None
}

#[inline]
fn ret(uc: &mut UserContext, value: i64) {
    uc.regs[reg::A0] = value as u64;
}

#[inline]
fn align_up(x: u64, align: u64) -> u64 {
    (x + align - 1) & !(align - 1)
}

#[inline]
fn add_signed(base: u64, delta: i64) -> Option<u64> {
    if delta >= 0 {
        base.checked_add(delta as u64)
    } else {
        base.checked_sub(delta.unsigned_abs())
    }
}
