use crate::abi::constants::{KERNEL_ELF_BASE, PADDR_BASE, PHYS_BASE_RAW, PPTR_BASE};
use crate::arch::x86_64::csr;
use crate::arch::x86_64::paging::{PTE_USER_RW, PTE_USER_RWX, PTE_USER_RX, PageTable};

pub const USER_ROOT_ENTRIES: usize = 256;
pub const USER_TOP: usize = USER_ROOT_ENTRIES << (12 + 9 * 2);

#[inline]
pub const fn pptr_to_paddr(vaddr: usize) -> usize {
    vaddr - PPTR_BASE + PADDR_BASE
}

#[inline]
pub const fn paddr_to_pptr(paddr: usize) -> usize {
    paddr + PPTR_BASE - PADDR_BASE
}

#[inline]
pub const fn paddr_to_mmio(paddr: usize) -> usize {
    paddr_to_pptr(paddr)
}

#[inline]
pub const fn kpptr_to_paddr(vaddr: usize) -> usize {
    if vaddr >= KERNEL_ELF_BASE {
        vaddr - KERNEL_ELF_BASE + PHYS_BASE_RAW
    } else {
        pptr_to_paddr(vaddr)
    }
}

#[inline]
pub const fn paddr_to_kpptr(paddr: usize) -> usize {
    paddr + KERNEL_ELF_BASE - PHYS_BASE_RAW
}

pub fn alloc_pt_page() -> *mut PageTable {
    panic!("x86_64 VSpace page-table allocation is not wired yet")
}

#[derive(Debug, Copy, Clone, Eq, PartialEq)]
pub enum UserMapError {
    InvalidArgument,
    FailedLookup(usize),
    DeleteFirst,
}

pub struct PreparedUserFrameMap {
    _private: (),
}

pub struct PreparedUserPageTableMap {
    mapped_addr: usize,
}

impl PreparedUserPageTableMap {
    #[inline]
    pub const fn mapped_addr(&self) -> usize {
        self.mapped_addr
    }
}

pub unsafe fn prepare_user_frame_map(
    _root: *mut PageTable,
    _vaddr: usize,
    _frame_paddr: usize,
    _size_class: u64,
    _flags: u64,
) -> Result<PreparedUserFrameMap, UserMapError> {
    Err(UserMapError::InvalidArgument)
}

pub unsafe fn prepare_user_frame_remap(
    _root: *mut PageTable,
    _vaddr: usize,
    _frame_paddr: usize,
    _size_class: u64,
    _flags: u64,
) -> Result<PreparedUserFrameMap, UserMapError> {
    Err(UserMapError::InvalidArgument)
}

pub unsafe fn commit_user_frame_map(_prepared: PreparedUserFrameMap) {}

pub unsafe fn prepare_user_page_table_map(
    _root: *mut PageTable,
    vaddr: usize,
    _pt_kva: *mut PageTable,
) -> Result<PreparedUserPageTableMap, UserMapError> {
    Ok(PreparedUserPageTableMap { mapped_addr: vaddr })
}

pub unsafe fn commit_user_page_table_map(_prepared: PreparedUserPageTableMap) {}

pub unsafe fn unmap_user_page_table(
    _root: *mut PageTable,
    _vaddr: usize,
    _target: *mut PageTable,
) -> bool {
    false
}

pub unsafe fn unmap_user_frame(
    _root: *mut PageTable,
    _vaddr: usize,
    _size_class: u64,
    _expected_pa: usize,
) -> Option<usize> {
    None
}

pub unsafe fn reclaim_user_page_tables(_root: *mut PageTable) {}

pub unsafe fn switch_satp(satp_val: u64) {
    unsafe {
        core::arch::asm!(
            "mov cr3, {}",
            in(reg) satp_val as usize,
            options(nostack, preserves_flags)
        );
    }
    csr::sfence_vma_all();
}

pub fn current_satp() -> u64 {
    let cr3: usize;
    unsafe {
        core::arch::asm!("mov {}, cr3", out(reg) cr3, options(nostack, preserves_flags));
    }
    cr3 as u64
}

pub fn set_current_vspace_root() {
    if let Some(kernel_satp) = crate::kernel::smp::kernel_satp() {
        if current_satp() != kernel_satp {
            unsafe { switch_satp(kernel_satp) };
        }
    }
}

pub fn user_flags(read: bool, write: bool, exec: bool) -> u64 {
    user_frame_flags(read, write, exec, false)
}

pub fn user_frame_flags(read: bool, write: bool, exec: bool, _is_device: bool) -> u64 {
    match (read, write, exec) {
        (true, true, true) => PTE_USER_RWX,
        (true, true, false) => PTE_USER_RW,
        (true, false, true) => PTE_USER_RX,
        _ => 0,
    }
}

pub fn copy_kernel_mappings_to(_pt: *mut PageTable) {}

pub fn make_boot_root_pt() -> *mut PageTable {
    panic!("x86_64 boot root page table is not wired yet")
}

pub fn satp_for(root: *mut PageTable, _asid: u64) -> u64 {
    kpptr_to_paddr(root as usize) as u64
}

pub fn satp_from_kva(root_kva: u64, _asid: u64) -> u64 {
    kpptr_to_paddr(root_kva as usize) as u64
}
