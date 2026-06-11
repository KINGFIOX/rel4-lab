#![no_std]
#![deny(unsafe_attr_outside_unsafe)]
#![deny(unsafe_op_in_unsafe_fn)]

pub const XV6_ABI_VERSION: u64 = 1;

/// xv6 user syscall numbers carried in the UnknownSyscall fault register set.
#[repr(u64)]
#[derive(Copy, Clone, Eq, PartialEq)]
pub enum Xv6Syscall {
    Fork = 1,
    Exit = 2,
    Wait = 3,
    Pipe = 4,
    Read = 5,
    Kill = 6,
    Exec = 7,
    Fstat = 8,
    Chdir = 9,
    Dup = 10,
    GetPid = 11,
    Sbrk = 12,
    Pause = 13,
    Uptime = 14,
    Open = 15,
    Write = 16,
    Mknod = 17,
    Unlink = 18,
    Link = 19,
    Mkdir = 20,
    Close = 21,
}

impl Xv6Syscall {
    pub const fn raw(self) -> u64 {
        self as u64
    }

    pub const fn from_raw(value: u64) -> Option<Self> {
        match value {
            1 => Some(Self::Fork),
            2 => Some(Self::Exit),
            3 => Some(Self::Wait),
            4 => Some(Self::Pipe),
            5 => Some(Self::Read),
            6 => Some(Self::Kill),
            7 => Some(Self::Exec),
            8 => Some(Self::Fstat),
            9 => Some(Self::Chdir),
            10 => Some(Self::Dup),
            11 => Some(Self::GetPid),
            12 => Some(Self::Sbrk),
            13 => Some(Self::Pause),
            14 => Some(Self::Uptime),
            15 => Some(Self::Open),
            16 => Some(Self::Write),
            17 => Some(Self::Mknod),
            18 => Some(Self::Unlink),
            19 => Some(Self::Link),
            20 => Some(Self::Mkdir),
            21 => Some(Self::Close),
            _ => None,
        }
    }
}

/// xv6 open flag bits. These values are ORed in the wire protocol.
#[repr(u32)]
#[derive(Copy, Clone, Eq, PartialEq)]
pub enum Xv6OpenFlag {
    WriteOnly = 0x001,
    ReadWrite = 0x002,
    Create = 0x200,
    Truncate = 0x400,
}

impl Xv6OpenFlag {
    pub const fn raw(self) -> u32 {
        self as u32
    }
}

/// xv6 inode type values exposed through stat/fs IPC.
#[repr(u16)]
#[derive(Copy, Clone, Eq, PartialEq)]
pub enum Xv6FileType {
    Directory = 1,
    File = 2,
    Device = 3,
}

impl Xv6FileType {
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
}

pub const ROOT_INO: u32 = 1;
pub const README_INO: u32 = 2;
pub const CONSOLE_INO: u32 = 3;

pub const DIRSIZ: usize = 14;
pub const DIRENT_SIZE: usize = 16;
pub const FS_BLOCK_SIZE: usize = 1024;
pub const XV6_MAX_FILE_WRITE: usize = ((10 - 1 - 1 - 2) / 2) * FS_BLOCK_SIZE;
pub const DIRENTS_PER_BLOCK: usize = FS_BLOCK_SIZE / DIRENT_SIZE;

pub const MAX_FD: usize = 32;
pub const MAX_PATH_BYTES: usize = 384;
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

/// Protocol tags stored in message register 0 for xv6 server IPC.
#[repr(u64)]
#[derive(Copy, Clone, Eq, PartialEq)]
pub enum Xv6Protocol {
    HostToVfs = 0x7836_7666_73,
    VfsToXv6Fs = 0x7836_7866_73,
    HostToVfsAsync = 0x7836_7666_7361,
    VfsToXv6FsAsync = 0x7836_7866_7361,
    VfsToUart = 0x7836_7561_72,
    VfsToUartAsync = 0x7836_7561_7261,
    FsToDisk = 0x7836_626c_6b,
}

impl Xv6Protocol {
    pub const fn raw(self) -> u64 {
        self as u64
    }

    pub const fn from_raw(value: u64) -> Option<Self> {
        match value {
            0x7836_7666_73 => Some(Self::HostToVfs),
            0x7836_7866_73 => Some(Self::VfsToXv6Fs),
            0x7836_7666_7361 => Some(Self::HostToVfsAsync),
            0x7836_7866_7361 => Some(Self::VfsToXv6FsAsync),
            0x7836_7561_72 => Some(Self::VfsToUart),
            0x7836_7561_7261 => Some(Self::VfsToUartAsync),
            0x7836_626c_6b => Some(Self::FsToDisk),
            _ => None,
        }
    }
}

pub const XV6_SERVICE_ENDPOINT_CPTR: u64 = 2;
pub const XV6_DISK_ENDPOINT_CPTR: u64 = 3;
pub const XV6_XV6FS_ENDPOINT_CPTR: u64 = 3;
pub const XV6_DISK_IRQ_NTFN_CPTR: u64 = 4;
pub const XV6_DISK_IRQ_HANDLER_CPTR: u64 = 5;
pub const XV6_SERVER_CNODE_CPTR: u64 = 6;
pub const XV6_SERVER_REPLY_CPTR: u64 = 7;
pub const XV6_UART_ENDPOINT_CPTR: u64 = 8;
pub const XV6_DISK_COMPLETION_NTFN_CPTR: u64 = 9;
pub const XV6_HOST_REPLY_ENDPOINT_CPTR: u64 = 10;
pub const XV6_VFS_REPLY_ENDPOINT_CPTR: u64 = 10;
pub const XV6_UART_REPLY_ENDPOINT_CPTR: u64 = 11;
pub const XV6_SERVER_RECV_REPLY_CPTR: u64 = 12;
pub const XV6_ASYNC_REPLY_CPTR_BASE: u64 = 32;

/// Endpoint/notification badges used by the xv6 server topology.
#[repr(u64)]
#[derive(Copy, Clone, Eq, PartialEq)]
pub enum Xv6Badge {
    VfsServer = 1 << 40,
    Xv6FsServer = 1 << 41,
    UartServer = 1 << 42,
    VfsReply = 1 << 43,
    Xv6FsReply = 1 << 44,
    UartReply = 1 << 45,
    DiskServer = 0x6469_736b,
    DiskIrq = 0x6469_7271,
    DiskCompletion = 0x6469_636d,
}

impl Xv6Badge {
    pub const fn raw(self) -> u64 {
        self as u64
    }
}

/// xv6 server status codes returned in message register 0.
#[repr(u64)]
#[derive(Copy, Clone, Eq, PartialEq)]
pub enum Xv6Status {
    Ok = 0,
    WouldBlock = 11,
    Busy = 16,
    InvalidArgument = 22,
    BrokenPipe = 32,
    NoSyscall = 38,
}

impl Xv6Status {
    pub const fn raw(self) -> u64 {
        self as u64
    }

    pub const fn from_raw(value: u64) -> Option<Self> {
        match value {
            0 => Some(Self::Ok),
            11 => Some(Self::WouldBlock),
            16 => Some(Self::Busy),
            22 => Some(Self::InvalidArgument),
            32 => Some(Self::BrokenPipe),
            38 => Some(Self::NoSyscall),
            _ => None,
        }
    }
}

/// Host -> VFS operation labels.
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
    Mknod = 12,
    Unlink = 13,
    Link = 14,
    Mkdir = 15,
    ExecOpen = 16,
    ExecRead = 17,
    ExecClose = 18,
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
            12 => Some(Self::Mknod),
            13 => Some(Self::Unlink),
            14 => Some(Self::Link),
            15 => Some(Self::Mkdir),
            16 => Some(Self::ExecOpen),
            17 => Some(Self::ExecRead),
            18 => Some(Self::ExecClose),
            _ => None,
        }
    }
}

/// VFS -> xv6fs operation labels.
#[repr(u64)]
#[derive(Copy, Clone, Eq, PartialEq)]
pub enum Xv6FsOp {
    Init = 0,
    OpenAt = 1,
    Retain = 2,
    Release = 3,
    Read = 4,
    Write = 5,
    ReadDir = 6,
    Fstat = 7,
    LookupDirectory = 8,
    Mknod = 9,
    Unlink = 10,
    Link = 11,
    Mkdir = 12,
}

impl Xv6FsOp {
    pub const fn raw(self) -> u64 {
        self as u64
    }

    pub const fn from_raw(value: u64) -> Option<Self> {
        match value {
            0 => Some(Self::Init),
            1 => Some(Self::OpenAt),
            2 => Some(Self::Retain),
            3 => Some(Self::Release),
            4 => Some(Self::Read),
            5 => Some(Self::Write),
            6 => Some(Self::ReadDir),
            7 => Some(Self::Fstat),
            8 => Some(Self::LookupDirectory),
            9 => Some(Self::Mknod),
            10 => Some(Self::Unlink),
            11 => Some(Self::Link),
            12 => Some(Self::Mkdir),
            _ => None,
        }
    }
}

/// VFS -> UART operation labels.
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

/// xv6fs -> disk operation labels.
#[repr(u64)]
#[derive(Copy, Clone, Eq, PartialEq)]
pub enum DiskRequestOp {
    GetInfo = 1,
    Read = 2,
    Write = 3,
    Flush = 4,
    Complete = 5,
}

impl DiskRequestOp {
    pub const fn raw(self) -> u64 {
        self as u64
    }

    pub const fn from_raw(value: u64) -> Option<Self> {
        match value {
            1 => Some(Self::GetInfo),
            2 => Some(Self::Read),
            3 => Some(Self::Write),
            4 => Some(Self::Flush),
            5 => Some(Self::Complete),
            _ => None,
        }
    }
}

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
pub const UART0_MMIO_BASE: u64 = 0x1000_0000;
pub const UART0_MMIO_SIZE: u64 = 0x1000;
pub const XV6_VIRTIO_MMIO_VADDR: u64 = 0x5000_0000;
pub const XV6_VIRTIO_DMA_VADDR: u64 = 0x5000_1000;
pub const XV6_DISK_SHARED_BUFFER_VADDR: u64 = 0x5000_2000;
pub const XV6_DISK_COMPLETION_RING_VADDR: u64 = 0x5000_3000;
pub const XV6_UART_MMIO_VADDR: u64 = 0x5000_4000;
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
pub const XV6_DISK_SHARED_BUFFER_SLOTS: usize = 16;
pub const XV6_DISK_SHARED_BUFFER_PAGES: usize = 4;
pub const XV6_HOST_SHARED_SLOT_BASE: usize = 0;
pub const XV6_HOST_SHARED_SLOT_COUNT: usize = 8;
pub const XV6_XV6FS_SCRATCH_SLOT_BASE: usize = 8;
pub const XV6_XV6FS_SCRATCH_SLOT_COUNT: usize = 8;

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

pub const fn pack_stat_type_nlink(typ: u16, nlink: u16) -> u64 {
    typ as u64 | ((nlink as u64) << 16)
}

pub const fn unpack_stat_type(value: u64) -> u16 {
    value as u16
}

pub const fn unpack_stat_nlink(value: u64) -> u16 {
    (value >> 16) as u16
}
