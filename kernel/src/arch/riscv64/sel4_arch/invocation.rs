//! RISC-V seL4 architecture invocation labels.
//!
//! Numbers match the non-MCS `arch_invocation_label` sequence used by this
//! project's existing user-space.

// This module is written entirely in terms of the safe abstractions in
// `ktypes`; keep it that way.
#![deny(unsafe_code)]

#[derive(Copy, Clone, Debug, Eq, PartialEq)]
#[repr(u64)]
pub enum ArchInvocation {
    PageTableMap = 33,
    PageTableUnmap = 34,
    PageMap = 35,
    PageUnmap = 36,
    PageGetAddress = 37,
    AsidControlMakePool = 38,
    AsidPoolAssign = 39,
    IrqIssueIrqHandlerTrigger = 40,
}

impl ArchInvocation {
    pub const fn raw(self) -> u64 {
        self as u64
    }
}

pub const PAGE_TABLE_MAP: u64 = ArchInvocation::PageTableMap.raw();
pub const PAGE_TABLE_UNMAP: u64 = ArchInvocation::PageTableUnmap.raw();
pub const PAGE_MAP: u64 = ArchInvocation::PageMap.raw();
pub const PAGE_UNMAP: u64 = ArchInvocation::PageUnmap.raw();
pub const PAGE_GET_ADDRESS: u64 = ArchInvocation::PageGetAddress.raw();
pub const ASID_CONTROL_MAKE_POOL: u64 = ArchInvocation::AsidControlMakePool.raw();
pub const ASID_POOL_ASSIGN: u64 = ArchInvocation::AsidPoolAssign.raw();
pub const IRQ_ISSUE_IRQ_HANDLER_TRIGGER: u64 = ArchInvocation::IrqIssueIrqHandlerTrigger.raw();

pub fn mapped_table_coverage_bits(label_id: u64) -> Option<usize> {
    if label_id == PAGE_TABLE_MAP {
        Some(0)
    } else {
        None
    }
}

pub fn is_mapped_table_unmap(label_id: u64) -> bool {
    label_id == PAGE_TABLE_UNMAP
}

pub fn io_port_in_reply_length(_label_id: u64) -> u64 {
    0
}
