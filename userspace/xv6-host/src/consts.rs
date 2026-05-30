pub(crate) const PAYLOAD_ELF: &[u8] = include_bytes!(env!("XV6_PAYLOAD_ELF"));
pub(crate) const ROOT_IS_INIT: bool = option_env!("XV6_COMPILED_ROOT_IS_INIT").is_some();
pub(crate) const README_BYTES: &[u8] = include_bytes!("../../../third_party/xv6-riscv/README");
pub(crate) const CONSOLE_INPUT: &[u8] = match option_env!("XV6_CONSOLE_INPUT") {
    Some(input) => input.as_bytes(),
    None => b"",
};
include!(env!("XV6_EXEC_CATALOG_RS"));

pub(crate) const PAGE_SIZE: u64 = 4096;
pub(crate) const ROOT_CNODE: u64 = 2;
pub(crate) const INIT_TCB: u64 = 1;
pub(crate) const INIT_VSPACE: u64 = 3;
pub(crate) const IRQ_CONTROL: u64 = 4;
pub(crate) const INIT_ASID_POOL: u64 = 6;
pub(crate) const ROOT_CNODE_DEPTH: u64 = 64;
pub(crate) const WORD_BITS: u64 = 64;

pub(crate) const SYS_CALL: isize = -1;
pub(crate) const SYS_REPLY_RECV: isize = -2;
pub(crate) const SYS_SEND: isize = -3;
pub(crate) const SYS_RECV: isize = -5;
pub(crate) const SYS_YIELD: isize = -7;
pub(crate) const SYS_DEBUG_PUT_CHAR: isize = -9;
pub(crate) const SYS_DEBUG_HALT: isize = -11;

pub(crate) const LABEL_UNTYPED_RETYPE: u64 = 1;
pub(crate) const LABEL_TCB_READ_REGISTERS: u64 = 2;
pub(crate) const LABEL_TCB_WRITE_REGISTERS: u64 = 3;
pub(crate) const LABEL_TCB_CONFIGURE: u64 = 5;
pub(crate) const LABEL_TCB_SET_PRIORITY: u64 = 6;
pub(crate) const LABEL_TCB_SUSPEND: u64 = 11;
pub(crate) const LABEL_TCB_BIND_NOTIFICATION: u64 = 13;
pub(crate) const LABEL_CNODE_REVOKE: u64 = 17;
pub(crate) const LABEL_CNODE_COPY: u64 = 20;
pub(crate) const LABEL_CNODE_MINT: u64 = 21;
pub(crate) const LABEL_CNODE_DELETE: u64 = 18;
pub(crate) const LABEL_CNODE_SAVE_CALLER: u64 = 25;
pub(crate) const LABEL_IRQ_ISSUE_IRQ_HANDLER: u64 = 26;
pub(crate) const LABEL_IRQ_SET_NOTIFICATION: u64 = 28;
pub(crate) const LABEL_RISCV_PAGE_MAP: u64 = 35;
pub(crate) const LABEL_RISCV_PAGE_UNMAP: u64 = 36;
pub(crate) const LABEL_RISCV_ASID_POOL_ASSIGN: u64 = 39;

pub(crate) const OBJ_UNTYPED: u64 = 0;
pub(crate) const OBJ_TCB: u64 = 1;
pub(crate) const OBJ_ENDPOINT: u64 = 2;
pub(crate) const OBJ_NOTIFICATION: u64 = 3;
pub(crate) const OBJ_CAP_TABLE: u64 = 4;
pub(crate) const OBJ_4K: u64 = 6;
pub(crate) const OBJ_PAGE_TABLE: u64 = 8;

pub(crate) const CHILD_CNODE_BITS: u64 = 8;
pub(crate) const CHILD_FAULT_EP: u64 = 1;
pub(crate) const CHILD_IPC_BUFFER: u64 = 0x7000_0000;
pub(crate) const CHILD_STACK_PAGES: usize = 1;
pub(crate) const XV6_MAXVA: u64 = 1 << (9 + 9 + 9 + 12 - 1);
pub(crate) const XV6_TRAMPOLINE: u64 = XV6_MAXVA - PAGE_SIZE;
pub(crate) const XV6_TRAPFRAME: u64 = XV6_TRAMPOLINE - PAGE_SIZE;
pub(crate) const CHILD_HEAP_LIMIT: u64 = XV6_TRAPFRAME;
pub(crate) const HOST_ALIAS_BASE: u64 = 0x4000_0000;
pub(crate) const MAX_MAPPINGS: usize = 8192;
pub(crate) const FRAME_POOL_CAP: usize = MAX_MAPPINGS;
pub(crate) const SBRK_MAPPING_HEADROOM: usize = 256;
pub(crate) const SBRK_EAGER_MAP_LIMIT: usize = 64;
pub(crate) const SPARSE_EAGER_RESERVE_LIMIT: u64 = 512 * 1024 * 1024;
pub(crate) const FORK_SLOT_HEADROOM: usize = 32;
pub(crate) const MAX_RECYCLED_SLOTS: usize = MAX_MAPPINGS * 2;
pub(crate) const MAX_PROCS: usize = 16;
pub(crate) const PROCESS_UNTYPED_BITS: u64 = 25;
pub(crate) const PROCESS_UNTYPED_PARENT_BITS: u8 = 29;
pub(crate) const MAX_FD: usize = 32;
pub(crate) const KERNEL_TIMER_IRQ: u64 = 96;

pub(crate) const PROC_UNUSED: u8 = 0;
pub(crate) const PROC_RUNNABLE: u8 = 1;
pub(crate) const PROC_ZOMBIE: u8 = 2;
pub(crate) const PROC_WAITING: u8 = 3;
pub(crate) const PROC_PIPE_WRITE: u8 = 4;

pub(crate) const FD_CLOSED: u8 = 0;
pub(crate) const FD_CONSOLE: u8 = 1;
pub(crate) const FD_FS_FILE: u8 = 2;
pub(crate) const FD_FS_DIR: u8 = 3;
pub(crate) const FD_PIPE_READ: u8 = 4;
pub(crate) const FD_PIPE_WRITE: u8 = 5;
pub(crate) const MAX_PIPES: usize = 8;
pub(crate) const PIPE_BUF: usize = 512;
pub(crate) const MAX_OPEN_FILES: usize = 128;
pub(crate) const MAX_EXEC_ARGS: usize = 16;
pub(crate) const MAX_EXEC_ARG_LEN: usize = 128;
pub(crate) const MAX_FS_NODES: usize = 256;
pub(crate) const MAX_DIR_ENTRIES: usize = 2048;
pub(crate) const FS_BLOCK_SIZE: usize = 1024;
pub(crate) const MAX_FILE_BLOCK_REFS: usize = 320;
pub(crate) const MAX_FILE_BLOCKS: usize = 2048;
pub(crate) const MAX_FILE_BYTES: usize = MAX_FILE_BLOCK_REFS * FS_BLOCK_SIZE;
pub(crate) const NO_FILE_BLOCK: u16 = u16::MAX;
pub(crate) const DIRSIZ: usize = 14;
pub(crate) const DIRENT_SIZE: usize = 16;
pub(crate) const DIRENTS_PER_BLOCK: usize = FS_BLOCK_SIZE / DIRENT_SIZE;

pub(crate) const FS_UNUSED: u8 = 0;
pub(crate) const FS_FILE: u8 = 1;
pub(crate) const FS_DIR: u8 = 2;
pub(crate) const FS_README: u8 = 3;
pub(crate) const FS_CONSOLE: u8 = 4;
pub(crate) const FS_EXEC: u8 = 5;

pub(crate) const FS_ROOT_NODE: usize = 0;
pub(crate) const FS_README_NODE: usize = 1;
pub(crate) const FS_CONSOLE_NODE: usize = 2;

pub(crate) const FAULT_UNKNOWN_SYSCALL: u64 = 2;
pub(crate) const FAULT_VM_FAULT: u64 = 5;

pub(crate) const SYS_FORK: u64 = 1;
pub(crate) const SYS_EXIT: u64 = 2;
pub(crate) const SYS_WAIT: u64 = 3;
pub(crate) const SYS_PIPE: u64 = 4;
pub(crate) const SYS_READ: u64 = 5;
pub(crate) const SYS_KILL: u64 = 6;
pub(crate) const SYS_EXEC: u64 = 7;
pub(crate) const SYS_FSTAT: u64 = 8;
pub(crate) const SYS_CHDIR: u64 = 9;
pub(crate) const SYS_DUP: u64 = 10;
pub(crate) const SYS_GETPID: u64 = 11;
pub(crate) const SYS_SBRK: u64 = 12;
pub(crate) const SYS_PAUSE: u64 = 13;
pub(crate) const SYS_UPTIME: u64 = 14;
pub(crate) const SYS_OPEN: u64 = 15;
pub(crate) const SYS_WRITE: u64 = 16;
pub(crate) const SYS_MKNOD: u64 = 17;
pub(crate) const SYS_UNLINK: u64 = 18;
pub(crate) const SYS_LINK: u64 = 19;
pub(crate) const SYS_MKDIR: u64 = 20;
pub(crate) const SYS_CLOSE: u64 = 21;

pub(crate) const SBRK_EAGER: u64 = 1;
pub(crate) const SBRK_LAZY: u64 = 2;

pub(crate) const O_WRONLY: u32 = 0x001;
pub(crate) const O_RDWR: u32 = 0x002;
pub(crate) const O_CREATE: u32 = 0x200;
pub(crate) const O_TRUNC: u32 = 0x400;

pub(crate) const T_DIR: u16 = 1;
pub(crate) const T_FILE: u16 = 2;
pub(crate) const T_DEVICE: u16 = 3;
pub(crate) const ROOT_INO: u32 = 1;
pub(crate) const README_INO: u32 = 2;
pub(crate) const CONSOLE_INO: u32 = 3;
