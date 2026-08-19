//! x86 seL4 architecture invocation labels.
//!
//! IDs follow the non-MCS common-label range, then the generated
//! `sel4_arch` / `arch` order used by libsel4 when SMP is off (no
//! `TCBSetAffinity`): PDPT, PageDirectory, PageTable, Page, ASID,
//! IoPort, then IRQControl GetIOAPIC/MSI.

#[derive(Copy, Clone, Debug, Eq, PartialEq)]
#[repr(u64)]
pub enum ArchInvocation {
    PdptMap = 33,
    PdptUnmap = 34,
    PageDirectoryMap = 35,
    PageDirectoryUnmap = 36,
    PageTableMap = 37,
    PageTableUnmap = 38,
    PageMap = 39,
    PageUnmap = 40,
    PageGetAddress = 41,
    AsidControlMakePool = 42,
    AsidPoolAssign = 43,
    IoPortControlIssue = 44,
    IoPortIn8 = 45,
    IoPortIn16 = 46,
    IoPortIn32 = 47,
    IoPortOut8 = 48,
    IoPortOut16 = 49,
    IoPortOut32 = 50,
    IrqIssueIrqHandlerIoapic = 51,
    IrqIssueIrqHandlerMsi = 52,
}

impl ArchInvocation {
    pub const fn raw(self) -> u64 {
        self as u64
    }
}

pub const PAGE_TABLE_MAP: u64 = ArchInvocation::PageTableMap.raw();
pub const PAGE_TABLE_UNMAP: u64 = ArchInvocation::PageTableUnmap.raw();
pub const PAGE_DIRECTORY_MAP: u64 = ArchInvocation::PageDirectoryMap.raw();
pub const PAGE_DIRECTORY_UNMAP: u64 = ArchInvocation::PageDirectoryUnmap.raw();
pub const PDPT_MAP: u64 = ArchInvocation::PdptMap.raw();
pub const PDPT_UNMAP: u64 = ArchInvocation::PdptUnmap.raw();
pub const PAGE_MAP: u64 = ArchInvocation::PageMap.raw();
pub const PAGE_UNMAP: u64 = ArchInvocation::PageUnmap.raw();
pub const PAGE_GET_ADDRESS: u64 = ArchInvocation::PageGetAddress.raw();
pub const ASID_CONTROL_MAKE_POOL: u64 = ArchInvocation::AsidControlMakePool.raw();
pub const ASID_POOL_ASSIGN: u64 = ArchInvocation::AsidPoolAssign.raw();
pub const IO_PORT_CONTROL_ISSUE: u64 = ArchInvocation::IoPortControlIssue.raw();
pub const IO_PORT_IN8: u64 = ArchInvocation::IoPortIn8.raw();
pub const IO_PORT_IN16: u64 = ArchInvocation::IoPortIn16.raw();
pub const IO_PORT_IN32: u64 = ArchInvocation::IoPortIn32.raw();
pub const IO_PORT_OUT8: u64 = ArchInvocation::IoPortOut8.raw();
pub const IO_PORT_OUT16: u64 = ArchInvocation::IoPortOut16.raw();
pub const IO_PORT_OUT32: u64 = ArchInvocation::IoPortOut32.raw();
pub const IRQ_ISSUE_IRQ_HANDLER_IOAPIC: u64 = ArchInvocation::IrqIssueIrqHandlerIoapic.raw();
pub const IRQ_ISSUE_IRQ_HANDLER_MSI: u64 = ArchInvocation::IrqIssueIrqHandlerMsi.raw();

/// Coverage of the table being inserted: PT→21, PD→30, PDPT→39.
/// `PML4_Map` is illegal in seL4 (the PML4 is the VSpace root).
pub fn mapped_table_coverage_bits(label_id: u64) -> Option<usize> {
    match label_id {
        PAGE_TABLE_MAP => Some(12 + 9),
        PAGE_DIRECTORY_MAP => Some(12 + 9 + 9),
        PDPT_MAP => Some(12 + 9 + 9 + 9),
        _ => None,
    }
}

pub fn is_mapped_table_unmap(label_id: u64) -> bool {
    matches!(
        label_id,
        PAGE_TABLE_UNMAP | PAGE_DIRECTORY_UNMAP | PDPT_UNMAP
    )
}

pub fn io_port_in_reply_length(label_id: u64) -> u64 {
    matches!(label_id, IO_PORT_IN8 | IO_PORT_IN16 | IO_PORT_IN32)
        .then_some(1)
        .unwrap_or(0)
}
