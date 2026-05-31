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
pub const XV6_MAX_FILE_WRITE: usize = ((10 - 1 - 1 - 2) / 2) * FS_BLOCK_SIZE;
pub const DIRENTS_PER_BLOCK: usize = FS_BLOCK_SIZE / DIRENT_SIZE;

pub const MAX_FD: usize = 32;
pub const MAX_PIPES: usize = 32;
pub const PIPE_BUF: usize = 512;
pub const MAX_OPEN_FILES: usize = 128;
pub const MAX_EXEC_ARGS: usize = 32;
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

pub const XV6_SERVICE_ENDPOINT_CPTR: u64 = 2;
pub const XV6_DISK_ENDPOINT_CPTR: u64 = 3;
pub const XV6_DISK_IRQ_NTFN_CPTR: u64 = 4;
pub const XV6_DISK_IRQ_HANDLER_CPTR: u64 = 5;
pub const XV6_SERVER_CNODE_CPTR: u64 = 6;
pub const XV6_SERVER_REPLY_CPTR: u64 = 7;
// Legacy name kept for older call sites; cptr 9 now carries the completion
// Notification send cap, not an endpoint cap.
pub const XV6_DISK_COMPLETION_ENDPOINT_CPTR: u64 = 9;
pub const XV6_DISK_COMPLETION_NTFN_CPTR: u64 = 9;
pub const XV6_FS_SERVER_BADGE: u64 = 0x6673;
pub const XV6_DISK_SERVER_BADGE: u64 = 0x6469_736b;
pub const XV6_DISK_IRQ_BADGE: u64 = 0x6469_7271;
pub const XV6_DISK_COMPLETION_BADGE: u64 = 0x6469_636d;

pub const XV6_OK: u64 = 0;
pub const XV6_EBUSY: u64 = 16;
pub const XV6_EINVAL: u64 = 22;
pub const XV6_ENOSYS: u64 = 38;

pub const FS_OP_INIT: u64 = 0;
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
pub const FS_OP_RETAIN: u64 = 13;

pub const DISK_OP_GET_INFO: u64 = 1;
pub const DISK_OP_READ: u64 = 2;
pub const DISK_OP_WRITE: u64 = 3;
pub const DISK_OP_FLUSH: u64 = 4;
pub const DISK_OP_COMPLETE: u64 = 5;

pub const VIRTIO_BLK_SECTOR_SIZE: usize = 512;
pub const XV6_FS_SECTORS_PER_BLOCK: usize = FS_BLOCK_SIZE / VIRTIO_BLK_SECTOR_SIZE;

pub const XV6_FS_MAGIC: u32 = 0x1020_3040;
pub const XV6_FS_SIZE_BLOCKS: u32 = 2000;
pub const XV6_FS_ROOT_INUM: u32 = 1;
pub const XV6_FS_NDIRECT: usize = 12;
pub const XV6_FS_NINDIRECT: usize = FS_BLOCK_SIZE / core::mem::size_of::<u32>();
pub const XV6_FS_MAXFILE_BLOCKS: usize = XV6_FS_NDIRECT + XV6_FS_NINDIRECT;

pub const VIRTIO_MMIO_BASE: u64 = 0x1000_1000;
pub const VIRTIO_MMIO_SIZE: u64 = 0x1000;
pub const VIRTIO0_IRQ: u64 = 1;
pub const XV6_VIRTIO_MMIO_VADDR: u64 = 0x5000_0000;
pub const XV6_VIRTIO_DMA_VADDR: u64 = 0x5000_1000;
pub const XV6_DISK_SHARED_BUFFER_VADDR: u64 = 0x5000_2000;
pub const XV6_DISK_COMPLETION_RING_VADDR: u64 = 0x5000_3000;
pub const XV6_DISK_COMPLETION_RING_ENTRIES: usize = 32;
pub const XV6_DISK_COMPLETION_ENTRY_WORDS: usize = 5;

pub const VIRTIO_MMIO_MAGIC_VALUE: u64 = 0x000;
pub const VIRTIO_MMIO_VERSION: u64 = 0x004;
pub const VIRTIO_MMIO_DEVICE_ID: u64 = 0x008;
pub const VIRTIO_MMIO_VENDOR_ID: u64 = 0x00c;
pub const VIRTIO_MMIO_DEVICE_FEATURES: u64 = 0x010;
pub const VIRTIO_MMIO_DRIVER_FEATURES: u64 = 0x020;
pub const VIRTIO_MMIO_QUEUE_SEL: u64 = 0x030;
pub const VIRTIO_MMIO_QUEUE_NUM_MAX: u64 = 0x034;
pub const VIRTIO_MMIO_QUEUE_NUM: u64 = 0x038;
pub const VIRTIO_MMIO_QUEUE_READY: u64 = 0x044;
pub const VIRTIO_MMIO_QUEUE_NOTIFY: u64 = 0x050;
pub const VIRTIO_MMIO_INTERRUPT_STATUS: u64 = 0x060;
pub const VIRTIO_MMIO_INTERRUPT_ACK: u64 = 0x064;
pub const VIRTIO_MMIO_STATUS: u64 = 0x070;
pub const VIRTIO_MMIO_QUEUE_DESC_LOW: u64 = 0x080;
pub const VIRTIO_MMIO_QUEUE_DESC_HIGH: u64 = 0x084;
pub const VIRTIO_MMIO_DRIVER_DESC_LOW: u64 = 0x090;
pub const VIRTIO_MMIO_DRIVER_DESC_HIGH: u64 = 0x094;
pub const VIRTIO_MMIO_DEVICE_DESC_LOW: u64 = 0x0a0;
pub const VIRTIO_MMIO_DEVICE_DESC_HIGH: u64 = 0x0a4;

pub const VIRTIO_CONFIG_S_ACKNOWLEDGE: u32 = 1;
pub const VIRTIO_CONFIG_S_DRIVER: u32 = 2;
pub const VIRTIO_CONFIG_S_DRIVER_OK: u32 = 4;
pub const VIRTIO_CONFIG_S_FEATURES_OK: u32 = 8;

pub const VIRTIO_BLK_F_RO: u32 = 5;
pub const VIRTIO_BLK_F_SCSI: u32 = 7;
pub const VIRTIO_BLK_F_FLUSH: u32 = 9;
pub const VIRTIO_BLK_F_CONFIG_WCE: u32 = 11;
pub const VIRTIO_BLK_F_MQ: u32 = 12;
pub const VIRTIO_F_ANY_LAYOUT: u32 = 27;
pub const VIRTIO_RING_F_INDIRECT_DESC: u32 = 28;
pub const VIRTIO_RING_F_EVENT_IDX: u32 = 29;

pub const VIRTIO_BLK_DEVICE_ID: u32 = 2;
pub const VIRTIO_MMIO_MAGIC: u32 = 0x7472_6976;
pub const VIRTIO_MMIO_VERSION_MODERN: u32 = 2;
pub const VIRTIO_MMIO_VENDOR_QEMU: u32 = 0x554d_4551;

pub const VIRTIO_BLK_T_IN: u32 = 0;
pub const VIRTIO_BLK_T_OUT: u32 = 1;
pub const VIRTIO_BLK_T_FLUSH: u32 = 4;
pub const VIRTQ_DESC_F_NEXT: u16 = 1;
pub const VIRTQ_DESC_F_WRITE: u16 = 2;
pub const VIRTIO_QUEUE_NUM: usize = 8;
pub const XV6_DISK_MAX_IN_FLIGHT: usize = 2;
pub const XV6_DISK_SHARED_BUFFER_SLOTS: usize = 4;

#[repr(C)]
#[derive(Copy, Clone)]
pub struct Xv6Superblock {
    pub magic: u32,
    pub size: u32,
    pub nblocks: u32,
    pub ninodes: u32,
    pub nlog: u32,
    pub logstart: u32,
    pub inodestart: u32,
    pub bmapstart: u32,
}

#[repr(C)]
#[derive(Copy, Clone)]
pub struct Xv6Dinode {
    pub typ: i16,
    pub major: i16,
    pub minor: i16,
    pub nlink: i16,
    pub size: u32,
    pub addrs: [u32; XV6_FS_NDIRECT + 1],
}

#[repr(C)]
#[derive(Copy, Clone)]
pub struct Xv6Dirent {
    pub inum: u16,
    pub name: [u8; DIRSIZ],
}

#[repr(C)]
#[derive(Copy, Clone)]
pub struct VirtqDesc {
    pub addr: u64,
    pub len: u32,
    pub flags: u16,
    pub next: u16,
}

#[repr(C)]
#[derive(Copy, Clone)]
pub struct VirtqAvail {
    pub flags: u16,
    pub idx: u16,
    pub ring: [u16; VIRTIO_QUEUE_NUM],
    pub unused: u16,
}

#[repr(C)]
#[derive(Copy, Clone)]
pub struct VirtqUsedElem {
    pub id: u32,
    pub len: u32,
}

#[repr(C)]
#[derive(Copy, Clone)]
pub struct VirtqUsed {
    pub flags: u16,
    pub idx: u16,
    pub ring: [VirtqUsedElem; VIRTIO_QUEUE_NUM],
}

#[repr(C)]
#[derive(Copy, Clone)]
pub struct VirtioBlkReq {
    pub typ: u32,
    pub reserved: u32,
    pub sector: u64,
}

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
