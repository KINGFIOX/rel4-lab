//! seL4 syscall numbers (passed in `a7` on RV64 RVI `ecall`).
//!
//! Mirror of [`kernel/libsel4/include/api/syscall.xml`] for the non-MCS
//! `api-master` configuration with `CONFIG_PRINTING` and
//! `CONFIG_DEBUG_BUILD` enabled.

#![allow(dead_code)]

pub type SyscallNo = isize;

// --- API (non-MCS, "api-master") ---
pub const SYS_CALL: SyscallNo = -1;
pub const SYS_REPLY_RECV: SyscallNo = -2;
pub const SYS_SEND: SyscallNo = -3;
pub const SYS_NB_SEND: SyscallNo = -4;
pub const SYS_RECV: SyscallNo = -5;
pub const SYS_REPLY: SyscallNo = -6;
pub const SYS_YIELD: SyscallNo = -7;
pub const SYS_NB_RECV: SyscallNo = -8;

// --- Debug syscalls (CONFIG_PRINTING / CONFIG_DEBUG_BUILD) ---
pub const SYS_DEBUG_PUT_CHAR: SyscallNo = -9;
pub const SYS_DEBUG_DUMP_SCHEDULER: SyscallNo = -10;
pub const SYS_DEBUG_HALT: SyscallNo = -11;
pub const SYS_DEBUG_CAP_IDENTIFY: SyscallNo = -12;
pub const SYS_DEBUG_SNAPSHOT: SyscallNo = -13;
pub const SYS_DEBUG_NAME_THREAD: SyscallNo = -14;
pub const SYS_DEBUG_SEND_IPI: SyscallNo = -15;
pub const SYS_DEBUG_GET_CHAR: SyscallNo = -16;

/// Returns true for syscall numbers we recognise in the current
/// configuration. Anything else triggers a kernel panic for now.
pub fn is_known(n: SyscallNo) -> bool {
    matches!(
        n,
        SYS_CALL
            | SYS_REPLY_RECV
            | SYS_SEND
            | SYS_NB_SEND
            | SYS_RECV
            | SYS_REPLY
            | SYS_YIELD
            | SYS_NB_RECV
            | SYS_DEBUG_PUT_CHAR
            | SYS_DEBUG_DUMP_SCHEDULER
            | SYS_DEBUG_HALT
            | SYS_DEBUG_CAP_IDENTIFY
            | SYS_DEBUG_SNAPSHOT
            | SYS_DEBUG_NAME_THREAD
            | SYS_DEBUG_SEND_IPI
            | SYS_DEBUG_GET_CHAR
    )
}
