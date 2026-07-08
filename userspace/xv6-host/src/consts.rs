pub(crate) const PAYLOAD_ELF: &[u8] = include_bytes!(env!("XV6_PAYLOAD_ELF"));
pub(crate) const UART_SERVER_ELF: &[u8] = include_bytes!(env!("XV6_UART_SERVER_ELF"));
pub(crate) const VFS_SERVER_ELF: &[u8] = include_bytes!(env!("XV6_VFS_SERVER_ELF"));
pub(crate) const XV6FS_SERVER_ELF: &[u8] = include_bytes!(env!("XV6_XV6FS_SERVER_ELF"));
pub(crate) const DISK_SERVER_ELF: &[u8] = include_bytes!(env!("XV6_DISK_SERVER_ELF"));
pub(crate) const ROOT_IS_INIT: bool = option_env!("XV6_COMPILED_ROOT_IS_INIT").is_some();

pub(crate) use sel4_user::{
    FAULT_UNKNOWN_SYSCALL, FAULT_VM_FAULT, INIT_ASID_POOL, INIT_TCB, INIT_VSPACE, IRQ_CONTROL,
    KERNEL_TIMER_IRQ, LABEL_ASID_POOL_ASSIGN, LABEL_CNODE_COPY, LABEL_CNODE_DELETE,
    LABEL_CNODE_MINT, LABEL_CNODE_REVOKE, LABEL_IRQ_ISSUE_IRQ_HANDLER, LABEL_IRQ_SET_NOTIFICATION,
    LABEL_PAGE_GET_ADDRESS, LABEL_PAGE_MAP, LABEL_PAGE_TABLE_MAP, LABEL_PAGE_UNMAP,
    LABEL_TCB_BIND_NOTIFICATION, LABEL_TCB_CONFIGURE, LABEL_TCB_READ_REGISTERS,
    LABEL_TCB_SET_SCHED_PARAMS, LABEL_TCB_SUSPEND, LABEL_TCB_WRITE_REGISTERS, LABEL_UNTYPED_RETYPE,
    OBJ_4K, OBJ_CAP_TABLE, OBJ_ENDPOINT, OBJ_NOTIFICATION, OBJ_PAGE_TABLE, OBJ_REPLY, OBJ_TCB,
    OBJ_UNTYPED, PAGE_SIZE, ROOT_CNODE, ROOT_CNODE_DEPTH, WORD_BITS,
};

pub(crate) const CHILD_CNODE_BITS: u64 = 8;
pub(crate) const CHILD_FAULT_EP: u64 = 1;
pub(crate) const CHILD_IPC_BUFFER: u64 = 0x7000_0000;
pub(crate) const CHILD_STACK_PAGES: usize = 1;
pub(crate) const SERVICE_STACK_PAGES: usize = 16;
pub(crate) const XV6_MAXVA: u64 = 1 << (9 + 9 + 9 + 12 - 1);
pub(crate) const XV6_TRAMPOLINE: u64 = XV6_MAXVA - PAGE_SIZE;
pub(crate) const XV6_TRAPFRAME: u64 = XV6_TRAMPOLINE - PAGE_SIZE;
pub(crate) const CHILD_HEAP_LIMIT: u64 = XV6_TRAPFRAME;
pub(crate) const HOST_ALIAS_BASE: u64 = 0x4000_0000;
pub(crate) const MAX_MAPPINGS: usize = 32768;
pub(crate) const MAX_PAGE_TABLE_MAPPINGS: usize = MAX_MAPPINGS + MAX_PROCS * 8;
pub(crate) const FRAME_POOL_CAP: usize = MAX_MAPPINGS;
pub(crate) const SBRK_MAPPING_HEADROOM: usize = 256;
pub(crate) const SBRK_EAGER_MAP_LIMIT: usize = 64;
pub(crate) const SPARSE_EAGER_RESERVE_LIMIT: u64 = 512 * 1024 * 1024;
pub(crate) const FORK_SLOT_HEADROOM: usize = 32;
pub(crate) const MAX_RECYCLED_SLOTS: usize = MAX_MAPPINGS * 2;
pub(crate) const MAX_PROCS: usize = 64;
pub(crate) const MAX_FAULT_REPLY_CAPS: usize = MAX_PROCS + 8;
pub(crate) const PROCESS_UNTYPED_BITS: u64 = 18;
pub(crate) const PROCESS_UNTYPED_PARENT_BITS: u8 = 29;
pub(crate) const PROC_UNUSED: u8 = 0;
pub(crate) const PROC_RUNNABLE: u8 = 1;
pub(crate) const PROC_ZOMBIE: u8 = 2;
pub(crate) const PROC_WAITING: u8 = 3;
pub(crate) const PROC_VFS_WRITE: u8 = 4;
pub(crate) const PROC_VFS_READ: u8 = 5;
pub(crate) const PROC_SLEEPING: u8 = 6;
pub(crate) const PROC_VFS_ASYNC: u8 = 7;
pub(crate) const PROC_VFS_DEFERRED: u8 = 8;
pub(crate) const SERVICE_UNTYPED_BITS: u64 = 22;
pub(crate) const UART_SERVER_PID: u64 = 0xffff_0000;
pub(crate) const VFS_SERVER_PID: u64 = 0xffff_0001;
pub(crate) const XV6FS_SERVER_PID: u64 = 0xffff_0002;
pub(crate) const DISK_SERVER_PID: u64 = 0xffff_0003;
pub(crate) const CHILD_PRIORITY: u64 = 255;
pub(crate) const CHILD_MCP: u64 = 255;
pub(crate) const SBRK_EAGER: u64 = 1;
pub(crate) const SBRK_LAZY: u64 = 2;
pub(crate) const VM_ATTR_UNCACHED: u64 = 1 << 1;

pub(crate) use xv6_abi::{
    FS_BLOCK_SIZE, MAX_EXEC_ARG_LEN, MAX_EXEC_ARGS, MAX_FD, MAX_FILE_BYTES, MAX_OPEN_FILES,
    MAX_PATH_BYTES, ROOT_INO, UART0_MMIO_FRAME_BASE, VIRTIO_BLK_SECTOR_SIZE, VfsOp,
    XV6_ABI_VERSION, XV6_DEVICE_MMIO_BASE, XV6_DEVICE_MMIO_SIZE, XV6_DISK_COMPLETION_NTFN_CPTR,
    XV6_DISK_COMPLETION_RING_VADDR, XV6_DISK_ENDPOINT_CPTR, XV6_DISK_IRQ_HANDLER_CPTR,
    XV6_DISK_IRQ_NTFN_CPTR, XV6_DISK_SHARED_BUFFER_PAGES, XV6_DISK_SHARED_BUFFER_VADDR,
    XV6_HOST_REPLY_ENDPOINT_CPTR, XV6_MAX_FILE_WRITE, XV6_SERVER_CNODE_CPTR,
    XV6_SERVER_RECV_REPLY_CPTR, XV6_SERVICE_ENDPOINT_CPTR, XV6_UART_ENDPOINT_CPTR,
    XV6_UART_MMIO_FRAME_VADDR, XV6_UART_REPLY_ENDPOINT_CPTR, XV6_VFS_REPLY_ENDPOINT_CPTR,
    XV6_VIRTIO_DMA_VADDR, XV6_XV6FS_ENDPOINT_CPTR, Xv6Badge, Xv6FileType, Xv6Protocol, Xv6Status,
    Xv6Syscall, unpack_stat_nlink, unpack_stat_type,
};
