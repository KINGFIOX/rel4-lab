use crate::arch::current as arch;
use crate::consts::{MAX_FD, MAX_OPEN_FILES, MAX_PATH_BYTES, PROC_UNUSED};

pub(crate) use sel4_user::BootInfo;

#[derive(Copy, Clone)]
pub(crate) struct Mapping {
    pub(crate) pid: u64,
    pub(crate) child_page: u64,
    pub(crate) alias_page: u64,
    pub(crate) frame_slot: u64,
    pub(crate) alias_slot: u64,
    pub(crate) writable: bool,
    pub(crate) executable: bool,
    pub(crate) pool_frame: bool,
}

pub(crate) enum SyscallResult {
    Reply(i64),
    ReplyFrame(arch::FaultReplyFrame),
    Block,
    Stop,
}

#[derive(Copy, Clone)]
pub(crate) struct TaskStruct {
    pub(crate) pid: u64,
    pub(crate) parent_pid: u64,
    pub(crate) state: u8,
    pub(crate) exit_status: i32,
    pub(crate) reparented_to_init: bool,
    pub(crate) tcb: u64,
    pub(crate) cnode: u64,
    pub(crate) vspace: u64,
    pub(crate) ipc_frame: u64,
    pub(crate) sched_context: u64,
    pub(crate) untyped: u64,
    pub(crate) fault_ep: u64,
    pub(crate) fault_ep_cap: u64,
    pub(crate) entry: u64,
    pub(crate) brk: u64,
    pub(crate) heap_start: u64,
    pub(crate) heap_mapped_end: u64,
    pub(crate) sparse_reserved: u64,
    pub(crate) cwd: [u8; MAX_PATH_BYTES],
    pub(crate) cwd_len: usize,
    pub(crate) cwd_inode: u32,
    pub(crate) fds: [usize; MAX_FD],
    pub(crate) fd_serial: [bool; MAX_FD],
    pub(crate) wait_status_ptr: u64,
    pub(crate) wait_reply_slot: u64,
    pub(crate) wait_reply_mrs: arch::FaultReplyFrame,
    pub(crate) vfs_reply_slot: u64,
    pub(crate) vfs_reply_mrs: arch::FaultReplyFrame,
    pub(crate) vfs_fd: usize,
    pub(crate) vfs_buf: u64,
    pub(crate) vfs_len: usize,
    pub(crate) vfs_done: usize,
    pub(crate) sleep_deadline: u64,
    pub(crate) sleep_reply_slot: u64,
    pub(crate) sleep_reply_mrs: arch::FaultReplyFrame,
    pub(crate) deferred_reply_slot: u64,
    pub(crate) deferred_mrs: [u64; 64],
}

impl TaskStruct {
    pub(crate) const fn empty() -> Self {
        Self {
            pid: 0,
            parent_pid: 0,
            state: PROC_UNUSED,
            exit_status: 0,
            reparented_to_init: false,
            tcb: 0,
            cnode: 0,
            vspace: 0,
            ipc_frame: 0,
            sched_context: 0,
            untyped: 0,
            fault_ep: 0,
            fault_ep_cap: 0,
            entry: 0,
            brk: 0,
            heap_start: 0,
            heap_mapped_end: 0,
            sparse_reserved: 0,
            cwd: {
                let mut cwd = [0u8; MAX_PATH_BYTES];
                cwd[0] = b'/';
                cwd
            },
            cwd_len: 1,
            cwd_inode: 0,
            fds: [MAX_OPEN_FILES; MAX_FD],
            fd_serial: [false; MAX_FD],
            wait_status_ptr: 0,
            wait_reply_slot: 0,
            wait_reply_mrs: [0; arch::FAULT_REPLY_WORDS],
            vfs_reply_slot: 0,
            vfs_reply_mrs: [0; arch::FAULT_REPLY_WORDS],
            vfs_fd: 0,
            vfs_buf: 0,
            vfs_len: 0,
            vfs_done: 0,
            sleep_deadline: 0,
            sleep_reply_slot: 0,
            sleep_reply_mrs: [0; arch::FAULT_REPLY_WORDS],
            deferred_reply_slot: 0,
            deferred_mrs: [0; 64],
        }
    }
}
