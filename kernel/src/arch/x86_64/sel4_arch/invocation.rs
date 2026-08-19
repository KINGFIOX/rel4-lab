//! x86 seL4 architecture invocation labels.
//!
//! IDs follow the non-MCS common-label range (1..32) used by this kernel,
//! then the x86 arch methods from `object-api-arch.xml` / `object-api-sel4-arch.xml`.

#[derive(Copy, Clone, Debug, Eq, PartialEq)]
#[repr(u64)]
pub enum ArchInvocation {
    PageDirectoryMap = 33,
    PageDirectoryUnmap = 34,
    PageTableMap = 35,
    PageTableUnmap = 36,
    PageMap = 37,
    PageUnmap = 38,
    PageGetAddress = 39,
    PdptMap = 40,
    PdptUnmap = 41,
    Pml4Map = 42,
    Pml4Unmap = 43,
    AsidControlMakePool = 44,
    AsidPoolAssign = 45,
    IrqIssueIrqHandlerTrigger = 46,
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
