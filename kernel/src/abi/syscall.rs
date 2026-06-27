//! seL4 syscall numbers (passed in `a7` on RV64 RVI `ecall`).
//!
//! Mirror of [`kernel/libsel4/include/api/syscall.xml`] for the non-MCS
//! `api-master` configuration with `CONFIG_PRINTING` and `CONFIG_DEBUG_BUILD`
//! enabled.

#![allow(dead_code)]

/// seL4 syscall numbers (non-MCS, "api-master").
#[repr(isize)]
#[derive(Copy, Clone, Debug, Eq, PartialEq)]
pub enum SyscallNumber {
    Call = -1,
    ReplyRecv = -2,
    Send = -3,
    NonBlockingSend = -4,
    Recv = -5,
    Reply = -6,
    Yield = -7,
    NonBlockingRecv = -8,
    DebugPutChar = -9,
    DebugDumpScheduler = -10,
    DebugHalt = -11,
    DebugCapIdentify = -12,
    DebugSnapshot = -13,
    DebugNameThread = -14,
    DebugSendIpi = -15,
}

impl SyscallNumber {
    pub const fn raw(self) -> isize {
        self as isize
    }

    pub const fn from_raw(value: isize) -> Option<Self> {
        match value {
            -1 => Some(Self::Call),
            -2 => Some(Self::ReplyRecv),
            -3 => Some(Self::Send),
            -4 => Some(Self::NonBlockingSend),
            -5 => Some(Self::Recv),
            -6 => Some(Self::Reply),
            -7 => Some(Self::Yield),
            -8 => Some(Self::NonBlockingRecv),
            -9 => Some(Self::DebugPutChar),
            -10 => Some(Self::DebugDumpScheduler),
            -11 => Some(Self::DebugHalt),
            -12 => Some(Self::DebugCapIdentify),
            -13 => Some(Self::DebugSnapshot),
            -14 => Some(Self::DebugNameThread),
            -15 => Some(Self::DebugSendIpi),
            _ => None,
        }
    }
}
