//! seL4 fault labels delivered through fault endpoint IPC.

#![allow(dead_code)]

#[derive(Copy, Clone, Debug, Eq, PartialEq)]
pub enum FaultLabel {
    CapFault,
    UnknownSyscall,
    UserException,
    VmFault,
}

impl FaultLabel {
    pub const fn raw(self) -> u64 {
        match self {
            Self::CapFault => 1,
            Self::UnknownSyscall => 2,
            Self::UserException => 3,
            Self::VmFault => 5,
        }
    }
}
