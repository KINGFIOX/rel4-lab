//! seL4 syscall numbers (passed in `a7` on RV64 RVI `ecall`).
//!
//! Mirror of [`kernel/libsel4/include/api/syscall.xml`] for the MCS
//! `api-master` configuration with `CONFIG_PRINTING` and
//! `CONFIG_DEBUG_BUILD` enabled.

#![allow(dead_code)]

/// seL4 syscall numbers (MCS, "api-master").
#[repr(isize)]
#[derive(Copy, Clone, Debug, Eq, PartialEq)]
pub enum SyscallNumber {
    Call = -1,
    ReplyRecv = -2,
    NonBlockingSendRecv = -3,
    NonBlockingSendWait = -4,
    Send = -5,
    NonBlockingSend = -6,
    Recv = -7,
    NonBlockingRecv = -8,
    Wait = -9,
    NonBlockingWait = -10,
    Yield = -11,
    DebugPutChar = -12,
    DebugDumpScheduler = -13,
    DebugHalt = -14,
    DebugCapIdentify = -15,
    DebugSnapshot = -16,
    DebugNameThread = -17,
    DebugSendIpi = -18,
}

impl SyscallNumber {
    pub const fn raw(self) -> isize {
        self as isize
    }

    pub const fn from_raw(value: isize) -> Option<Self> {
        match value {
            -1 => Some(Self::Call),
            -2 => Some(Self::ReplyRecv),
            -3 => Some(Self::NonBlockingSendRecv),
            -4 => Some(Self::NonBlockingSendWait),
            -5 => Some(Self::Send),
            -6 => Some(Self::NonBlockingSend),
            -7 => Some(Self::Recv),
            -8 => Some(Self::NonBlockingRecv),
            -9 => Some(Self::Wait),
            -10 => Some(Self::NonBlockingWait),
            -11 => Some(Self::Yield),
            -12 => Some(Self::DebugPutChar),
            -13 => Some(Self::DebugDumpScheduler),
            -14 => Some(Self::DebugHalt),
            -15 => Some(Self::DebugCapIdentify),
            -16 => Some(Self::DebugSnapshot),
            -17 => Some(Self::DebugNameThread),
            -18 => Some(Self::DebugSendIpi),
            _ => None,
        }
    }
}
