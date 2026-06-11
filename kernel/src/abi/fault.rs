//! seL4 fault labels delivered through fault endpoint IPC.

#![allow(dead_code)]

#[repr(u64)]
#[derive(Copy, Clone, Debug, Eq, PartialEq)]
pub enum FaultLabel {
    CapFault = 1,
    UnknownSyscall = 2,
    UserException = 3,
    Timeout = 5,
    VmFault = 6,
}

impl FaultLabel {
    pub const fn raw(self) -> u64 {
        self as u64
    }
}
