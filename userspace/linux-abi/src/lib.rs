#![no_std]
#![deny(unsafe_attr_outside_unsafe)]
#![deny(unsafe_op_in_unsafe_fn)]

pub mod platform;
pub use platform::UartMmio;
pub use platform::current::*;

pub const LINUX_ABI_VERSION: u64 = 1;
pub const PAGE_SIZE: usize = 4096;

// Linux RV64 syscall numbers (asm-generic / riscv64).
pub const SYS_GETCWD: u64 = 17;
pub const SYS_DUP: u64 = 23;
pub const SYS_DUP3: u64 = 24;
pub const SYS_FCNTL: u64 = 25;
pub const SYS_IOCTL: u64 = 29;
pub const SYS_MKNODAT: u64 = 33;
pub const SYS_MKDIRAT: u64 = 34;
pub const SYS_UNLINKAT: u64 = 35;
pub const SYS_LINKAT: u64 = 37;
pub const SYS_CHDIR: u64 = 49;
pub const SYS_FCHDIR: u64 = 50;
pub const SYS_FACCESSAT: u64 = 48;
pub const SYS_OPENAT: u64 = 56;
pub const SYS_CLOSE: u64 = 57;
pub const SYS_PIPE2: u64 = 59;
pub const SYS_GETDENTS64: u64 = 61;
pub const SYS_LSEEK: u64 = 62;
pub const SYS_READ: u64 = 63;
pub const SYS_WRITE: u64 = 64;
pub const SYS_READV: u64 = 65;
pub const SYS_WRITEV: u64 = 66;
pub const SYS_PPOLL: u64 = 73;
pub const SYS_READLINKAT: u64 = 78;
pub const SYS_NEWFSTATAT: u64 = 79;
pub const SYS_FSTAT: u64 = 80;
pub const SYS_EXIT: u64 = 93;
pub const SYS_EXIT_GROUP: u64 = 94;
pub const SYS_WAITID: u64 = 95;
pub const SYS_SET_TID_ADDRESS: u64 = 96;
pub const SYS_SET_ROBUST_LIST: u64 = 99;
pub const SYS_GET_ROBUST_LIST: u64 = 100;
pub const SYS_NANOSLEEP: u64 = 101;
pub const SYS_CLOCK_GETTIME: u64 = 113;
pub const SYS_CLOCK_GETRES: u64 = 114;
pub const SYS_CLOCK_NANOSLEEP: u64 = 115;
pub const SYS_SCHED_YIELD: u64 = 124;
pub const SYS_KILL: u64 = 129;
pub const SYS_TKILL: u64 = 130;
pub const SYS_TGKILL: u64 = 131;
pub const SYS_RT_SIGACTION: u64 = 134;
pub const SYS_RT_SIGPROCMASK: u64 = 135;
pub const SYS_RT_SIGRETURN: u64 = 139;
pub const SYS_UNAME: u64 = 160;
pub const SYS_GETRUSAGE: u64 = 165;
pub const SYS_UMASK: u64 = 166;
pub const SYS_PRCTL: u64 = 167;
pub const SYS_GETTIMEOFDAY: u64 = 169;
pub const SYS_GETPID: u64 = 172;
pub const SYS_GETPPID: u64 = 173;
pub const SYS_GETUID: u64 = 174;
pub const SYS_GETEUID: u64 = 175;
pub const SYS_GETGID: u64 = 176;
pub const SYS_GETEGID: u64 = 177;
pub const SYS_GETTID: u64 = 178;
pub const SYS_SYSINFO: u64 = 179;
pub const SYS_BRK: u64 = 214;
pub const SYS_MUNMAP: u64 = 215;
pub const SYS_MREMAP: u64 = 216;
pub const SYS_CLONE: u64 = 220;
pub const SYS_EXECVE: u64 = 221;
pub const SYS_MMAP: u64 = 222;
pub const SYS_MPROTECT: u64 = 226;
pub const SYS_WAIT4: u64 = 260;
pub const SYS_PRLIMIT64: u64 = 261;
pub const SYS_GETRANDOM: u64 = 278;
pub const SYS_STATX: u64 = 291;
pub const SYS_CLONE3: u64 = 435;
pub const SYS_FUTEX: u64 = 98;
pub const SYS_SOCKET: u64 = 198;
pub const SYS_PTRACE: u64 = 117;

pub const EPERM: i32 = 1;
pub const ENOENT: i32 = 2;
pub const ESRCH: i32 = 3;
pub const EINTR: i32 = 4;
pub const EIO: i32 = 5;
pub const ENXIO: i32 = 6;
pub const E2BIG: i32 = 7;
pub const ENOEXEC: i32 = 8;
pub const EBADF: i32 = 9;
pub const ECHILD: i32 = 10;
pub const EAGAIN: i32 = 11;
pub const ENOMEM: i32 = 12;
pub const EACCES: i32 = 13;
pub const EFAULT: i32 = 14;
pub const EBUSY: i32 = 16;
pub const EEXIST: i32 = 17;
pub const EXDEV: i32 = 18;
pub const ENODEV: i32 = 19;
pub const ENOTDIR: i32 = 20;
pub const EISDIR: i32 = 21;
pub const EINVAL: i32 = 22;
pub const ENFILE: i32 = 23;
pub const EMFILE: i32 = 24;
pub const ENOTTY: i32 = 25;
pub const ENOSPC: i32 = 28;
pub const ESPIPE: i32 = 29;
pub const EROFS: i32 = 30;
pub const EPIPE: i32 = 32;
pub const ERANGE: i32 = 34;
pub const ENAMETOOLONG: i32 = 36;
pub const ENOSYS: i32 = 38;
pub const ENOTEMPTY: i32 = 39;

pub const O_RDONLY: u32 = 0;
pub const O_WRONLY: u32 = 1;
pub const O_RDWR: u32 = 2;
pub const O_ACCMODE: u32 = 3;
pub const O_CREAT: u32 = 0o100;
pub const O_EXCL: u32 = 0o200;
pub const O_NOCTTY: u32 = 0o400;
pub const O_TRUNC: u32 = 0o1000;
pub const O_APPEND: u32 = 0o2000;
pub const O_NONBLOCK: u32 = 0o4000;
pub const O_DIRECTORY: u32 = 0o200000;
pub const O_CLOEXEC: u32 = 0o2000000;

pub const AT_FDCWD: i32 = -100;
pub const AT_REMOVEDIR: u32 = 0x200;
pub const AT_SYMLINK_NOFOLLOW: u32 = 0x100;

pub const SEEK_SET: u32 = 0;
pub const SEEK_CUR: u32 = 1;
pub const SEEK_END: u32 = 2;

pub const S_IFMT: u32 = 0o170000;
pub const S_IFDIR: u32 = 0o040000;
pub const S_IFCHR: u32 = 0o020000;
pub const S_IFREG: u32 = 0o100000;
pub const S_IFIFO: u32 = 0o010000;

pub const CLOCK_REALTIME: u64 = 0;
pub const CLOCK_MONOTONIC: u64 = 1;

pub const WNOHANG: u32 = 1;
pub const WUNTRACED: u32 = 2;

pub const P_ALL: u32 = 0;
pub const P_PID: u32 = 1;
pub const P_PGID: u32 = 2;
pub const P_PIDFD: u32 = 3;

pub const CLD_EXITED: i32 = 1;
pub const CLD_KILLED: i32 = 2;

pub const SIGCHLD: u64 = 17;
pub const CLONE_VM: u64 = 0x0000_0100;
pub const CLONE_FS: u64 = 0x0000_0200;
pub const CLONE_FILES: u64 = 0x0000_0400;
pub const CLONE_SIGHAND: u64 = 0x0000_0800;
pub const CLONE_THREAD: u64 = 0x0001_0000;
pub const CLONE_CHILD_CLEARTID: u64 = 0x0020_0000;
pub const CLONE_CHILD_SETTID: u64 = 0x0100_0000;

pub const MAP_SHARED: u32 = 0x01;
pub const MAP_PRIVATE: u32 = 0x02;
pub const MAP_FIXED: u32 = 0x10;
pub const MAP_ANONYMOUS: u32 = 0x20;

pub const PROT_READ: u32 = 1;
pub const PROT_WRITE: u32 = 2;
pub const PROT_EXEC: u32 = 4;

pub const AT_NULL: u64 = 0;
pub const AT_PHDR: u64 = 3;
pub const AT_PHENT: u64 = 4;
pub const AT_PHNUM: u64 = 5;
pub const AT_PAGESZ: u64 = 6;
pub const AT_BASE: u64 = 7;
pub const AT_FLAGS: u64 = 8;
pub const AT_ENTRY: u64 = 9;
pub const AT_UID: u64 = 11;
pub const AT_EUID: u64 = 12;
pub const AT_GID: u64 = 13;
pub const AT_EGID: u64 = 14;
pub const AT_SECURE: u64 = 23;
pub const AT_RANDOM: u64 = 25;
pub const AT_EXECFN: u64 = 31;
pub const AT_HWCAP: u64 = 16;
pub const AT_CLKTCK: u64 = 17;

pub const ROOT_INO: u32 = 1;
pub const CONSOLE_INO: u32 = 3;

pub const MAX_FD: usize = 64;
pub const MAX_PATH_BYTES: usize = 384;
pub const MAX_PIPES: usize = 32;
pub const PIPE_BUF: usize = 512;
pub const MAX_OPEN_FILES: usize = 128;
pub const MAX_EXEC_ARGS: usize = 32;
pub const MAX_EXEC_ARG_LEN: usize = 128;
pub const MAX_FILE_BYTES: usize = 256 * 1024;
pub const MAX_IO_BYTES: usize = 16 * 1024;
pub const SHARED_BUFFER_PAGES: usize = 4;
pub const SHARED_BUFFER_VADDR: u64 = 0x5000_2000;

pub const SERVICE_ENDPOINT_CPTR: u64 = 2;
pub const SERVER_CNODE_CPTR: u64 = 6;
pub const SERVER_REPLY_CPTR: u64 = 7;
pub const UART_ENDPOINT_CPTR: u64 = 8;
pub const HOST_REPLY_ENDPOINT_CPTR: u64 = 10;
pub const UART_REPLY_ENDPOINT_CPTR: u64 = 11;
pub const SERVER_RECV_REPLY_CPTR: u64 = 12;

/// Protocol tags stored in message register 0.
#[repr(u64)]
#[derive(Copy, Clone, Eq, PartialEq)]
pub enum IpcProtocol {
    HostToVfs = 0x6c78_7666_73,
    HostToVfsAsync = 0x6c78_7666_7361,
    VfsToUart = 0x6c78_7561_72,
    VfsToUartAsync = 0x6c78_7561_7261,
}

impl IpcProtocol {
    pub const fn raw(self) -> u64 {
        self as u64
    }
}

#[repr(u64)]
#[derive(Copy, Clone, Eq, PartialEq)]
pub enum IpcBadge {
    VfsServer = 1 << 40,
    UartServer = 1 << 42,
    VfsReply = 1 << 43,
    UartReply = 1 << 45,
}

impl IpcBadge {
    pub const fn raw(self) -> u64 {
        self as u64
    }
}

/// VFS/UART status codes. 0 is success; other values are Linux errno.
#[repr(u64)]
#[derive(Copy, Clone, Eq, PartialEq)]
pub enum IpcStatus {
    Ok = 0,
    WouldBlock = 11,
    Busy = 16,
    InvalidArgument = 22,
    BrokenPipe = 32,
    NoSyscall = 38,
}

impl IpcStatus {
    pub const fn raw(self) -> u64 {
        self as u64
    }
}

#[repr(u64)]
#[derive(Copy, Clone, Eq, PartialEq)]
pub enum VfsOp {
    Init = 0,
    ProcInit = 1,
    ProcFork = 2,
    ProcExit = 3,
    Open = 4,
    Close = 5,
    Dup = 6,
    Read = 7,
    Write = 8,
    Fstat = 9,
    Chdir = 10,
    Pipe = 11,
    Unlink = 12,
    Mkdir = 13,
    ExecOpen = 14,
    ExecRead = 15,
    ExecClose = 16,
    Getcwd = 17,
    Lseek = 18,
}

impl VfsOp {
    pub const fn raw(self) -> u64 {
        self as u64
    }

    pub const fn from_raw(value: u64) -> Option<Self> {
        match value {
            0 => Some(Self::Init),
            1 => Some(Self::ProcInit),
            2 => Some(Self::ProcFork),
            3 => Some(Self::ProcExit),
            4 => Some(Self::Open),
            5 => Some(Self::Close),
            6 => Some(Self::Dup),
            7 => Some(Self::Read),
            8 => Some(Self::Write),
            9 => Some(Self::Fstat),
            10 => Some(Self::Chdir),
            11 => Some(Self::Pipe),
            12 => Some(Self::Unlink),
            13 => Some(Self::Mkdir),
            14 => Some(Self::ExecOpen),
            15 => Some(Self::ExecRead),
            16 => Some(Self::ExecClose),
            17 => Some(Self::Getcwd),
            18 => Some(Self::Lseek),
            _ => None,
        }
    }
}

#[repr(u64)]
#[derive(Copy, Clone, Eq, PartialEq)]
pub enum UartOp {
    Init = 0,
    PutChar = 1,
    GetChar = 2,
}

impl UartOp {
    pub const fn raw(self) -> u64 {
        self as u64
    }

    pub const fn from_raw(value: u64) -> Option<Self> {
        match value {
            0 => Some(Self::Init),
            1 => Some(Self::PutChar),
            2 => Some(Self::GetChar),
            _ => None,
        }
    }
}

#[repr(u16)]
#[derive(Copy, Clone, Eq, PartialEq)]
pub enum FileKind {
    Directory = 1,
    File = 2,
    Device = 3,
}

impl FileKind {
    pub const fn raw(self) -> u16 {
        self as u16
    }

    pub const fn from_raw(value: u16) -> Option<Self> {
        match value {
            1 => Some(Self::Directory),
            2 => Some(Self::File),
            3 => Some(Self::Device),
            _ => None,
        }
    }

    pub const fn mode(self) -> u32 {
        match self {
            Self::Directory => S_IFDIR | 0o755,
            Self::File => S_IFREG | 0o644,
            Self::Device => S_IFCHR | 0o666,
        }
    }
}

pub const fn linux_exit_status(status: i32) -> i32 {
    (status & 0xff) << 8
}

pub const fn wants_write(flags: u32) -> bool {
    flags & (O_WRONLY | O_RDWR | O_CREAT | O_TRUNC) != 0
}

pub const fn open_readable(flags: u32) -> bool {
    (flags & O_ACCMODE) != O_WRONLY
}

pub const fn open_writable(flags: u32) -> bool {
    (flags & O_ACCMODE) == O_WRONLY || (flags & O_ACCMODE) == O_RDWR
}

#[repr(C)]
#[derive(Copy, Clone)]
pub struct LinuxTimespec {
    pub tv_sec: i64,
    pub tv_nsec: i64,
}

#[repr(C)]
#[derive(Copy, Clone)]
pub struct LinuxTimeval {
    pub tv_sec: i64,
    pub tv_usec: i64,
}

pub const UTS_LEN: usize = 65;

#[repr(C)]
#[derive(Copy, Clone)]
pub struct LinuxUtsname {
    pub sysname: [u8; UTS_LEN],
    pub nodename: [u8; UTS_LEN],
    pub release: [u8; UTS_LEN],
    pub version: [u8; UTS_LEN],
    pub machine: [u8; UTS_LEN],
    pub domainname: [u8; UTS_LEN],
}

pub const LINUX_STAT_SIZE: usize = 128;

pub const fn pack_stat_kind_nlink(kind: u16, nlink: u16) -> u64 {
    kind as u64 | ((nlink as u64) << 16)
}

pub const fn unpack_stat_kind(value: u64) -> u16 {
    value as u16
}

pub const fn unpack_stat_nlink(value: u64) -> u16 {
    (value >> 16) as u16
}
