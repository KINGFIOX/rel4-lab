#![no_std]
#![deny(unsafe_attr_outside_unsafe)]
#![deny(unsafe_op_in_unsafe_fn)]

pub const XV6_ABI_VERSION: u64 = 1;

pub const SYS_FORK: u64 = 1;
pub const SYS_EXIT: u64 = 2;
pub const SYS_WAIT: u64 = 3;
pub const SYS_PIPE: u64 = 4;
pub const SYS_READ: u64 = 5;
pub const SYS_KILL: u64 = 6;
pub const SYS_EXEC: u64 = 7;
pub const SYS_FSTAT: u64 = 8;
pub const SYS_CHDIR: u64 = 9;
pub const SYS_DUP: u64 = 10;
pub const SYS_GETPID: u64 = 11;
pub const SYS_SBRK: u64 = 12;
pub const SYS_PAUSE: u64 = 13;
pub const SYS_UPTIME: u64 = 14;
pub const SYS_OPEN: u64 = 15;
pub const SYS_WRITE: u64 = 16;
pub const SYS_MKNOD: u64 = 17;
pub const SYS_UNLINK: u64 = 18;
pub const SYS_LINK: u64 = 19;
pub const SYS_MKDIR: u64 = 20;
pub const SYS_CLOSE: u64 = 21;

pub const O_WRONLY: u32 = 0x001;
pub const O_RDWR: u32 = 0x002;
pub const O_CREATE: u32 = 0x200;
pub const O_TRUNC: u32 = 0x400;

pub const T_DIR: u16 = 1;
pub const T_FILE: u16 = 2;
pub const T_DEVICE: u16 = 3;

pub const ROOT_INO: u32 = 1;
pub const README_INO: u32 = 2;
pub const CONSOLE_INO: u32 = 3;

pub const DIRSIZ: usize = 14;
pub const DIRENT_SIZE: usize = 16;
pub const FS_BLOCK_SIZE: usize = 1024;
pub const DIRENTS_PER_BLOCK: usize = FS_BLOCK_SIZE / DIRENT_SIZE;

pub const MAX_FD: usize = 32;
pub const MAX_PIPES: usize = 8;
pub const PIPE_BUF: usize = 512;
pub const MAX_OPEN_FILES: usize = 128;
pub const MAX_EXEC_ARGS: usize = 16;
pub const MAX_EXEC_ARG_LEN: usize = 128;
pub const MAX_FS_NODES: usize = 256;
pub const MAX_DIR_ENTRIES: usize = 2048;
pub const MAX_FILE_BLOCK_REFS: usize = 320;
pub const MAX_FILE_BLOCKS: usize = 2048;
pub const MAX_FILE_BYTES: usize = MAX_FILE_BLOCK_REFS * FS_BLOCK_SIZE;
pub const NO_FILE_BLOCK: u16 = u16::MAX;

pub const FS_UNUSED: u8 = 0;
pub const FS_FILE: u8 = 1;
pub const FS_DIR: u8 = 2;
pub const FS_README: u8 = 3;
pub const FS_CONSOLE: u8 = 4;
pub const FS_EXEC: u8 = 5;

pub const FS_ROOT_NODE: usize = 0;
pub const FS_README_NODE: usize = 1;
pub const FS_CONSOLE_NODE: usize = 2;

pub const XV6_HOST_TO_FS_PROTOCOL: u64 = 0x7836_6673;
pub const XV6_FS_TO_DISK_PROTOCOL: u64 = 0x7836_626c_6b;

pub const FS_OP_OPEN: u64 = 1;
pub const FS_OP_CLOSE: u64 = 2;
pub const FS_OP_READ: u64 = 3;
pub const FS_OP_WRITE: u64 = 4;
pub const FS_OP_FSTAT: u64 = 5;
pub const FS_OP_CHDIR: u64 = 6;
pub const FS_OP_MKNOD: u64 = 7;
pub const FS_OP_UNLINK: u64 = 8;
pub const FS_OP_LINK: u64 = 9;
pub const FS_OP_MKDIR: u64 = 10;
pub const FS_OP_EXEC_LOOKUP: u64 = 11;
pub const FS_OP_READDIR: u64 = 12;

pub const DISK_OP_GET_INFO: u64 = 1;
pub const DISK_OP_READ: u64 = 2;
pub const DISK_OP_WRITE: u64 = 3;
pub const DISK_OP_FLUSH: u64 = 4;

pub const VIRTIO_BLK_SECTOR_SIZE: usize = 512;
pub const XV6_FS_SECTORS_PER_BLOCK: usize = FS_BLOCK_SIZE / VIRTIO_BLK_SECTOR_SIZE;

#[repr(C)]
#[derive(Copy, Clone)]
pub struct Xv6Stat {
    pub dev: i32,
    pub ino: u32,
    pub typ: u16,
    pub nlink: u16,
    pub size: u64,
}

impl Xv6Stat {
    pub const fn new(dev: i32, ino: u32, typ: u16, nlink: u16, size: u64) -> Self {
        Self {
            dev,
            ino,
            typ,
            nlink,
            size,
        }
    }
}
