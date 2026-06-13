//! LoongArch64 VSpace backend skeleton.

use crate::abi::constants::{KERNEL_ELF_BASE, PADDR_BASE, PHYS_BASE_RAW, PPTR_BASE};
use crate::arch::loongarch64::paging::{
    PAGE_SIZE, PTE_A, PTE_D, PTE_R, PTE_U, PTE_V, PTE_W, PTE_X, PageTable, Pte,
};
use crate::kernel::smp::{BklCell, BklObjectGuard};

type VspaceLockGuard = BklObjectGuard;

#[inline]
fn lock_vspace(_root: *const PageTable) -> VspaceLockGuard {
    BklObjectGuard::new()
}

#[inline]
pub const fn pptr_to_paddr(vaddr: usize) -> usize {
    vaddr - PPTR_BASE + PADDR_BASE
}

#[inline]
pub const fn paddr_to_pptr(paddr: usize) -> usize {
    paddr - PADDR_BASE + PPTR_BASE
}

#[inline]
pub const fn kpptr_to_paddr(vaddr: usize) -> usize {
    vaddr - KERNEL_ELF_BASE + PHYS_BASE_RAW
}

#[inline]
pub const fn paddr_to_kpptr(paddr: usize) -> usize {
    paddr - PHYS_BASE_RAW + KERNEL_ELF_BASE
}

const BOOT_PT_POOL_PAGES: usize = 1024;

#[repr(C, align(4096))]
struct BootPtPool {
    pages: [PageTable; BOOT_PT_POOL_PAGES],
    next: usize,
}

impl BootPtPool {
    const fn new() -> Self {
        Self {
            pages: [const { PageTable::zeroed() }; BOOT_PT_POOL_PAGES],
            next: 0,
        }
    }
}

static BOOT_PT_POOL: BklCell<BootPtPool> = BklCell::new(BootPtPool::new());

pub fn alloc_pt_page() -> *mut PageTable {
    BOOT_PT_POOL.with_mut(|pool| {
        let idx = pool.next;
        assert!(idx < BOOT_PT_POOL_PAGES, "boot PT pool exhausted");
        pool.next += 1;
        unsafe {
            let p = pool.pages.as_mut_ptr().add(idx);
            (*p).entries = [Pte::NULL; 512];
            p
        }
    })
}

#[derive(Copy, Clone, Debug, Eq, PartialEq)]
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
    _paddr: usize,
    _size_class: u64,
    _flags: u64,
) -> Result<PreparedUserFrameMap, UserMapError> {
    Err(UserMapError::InvalidArgument)
}

pub unsafe fn prepare_user_frame_remap(
    _root: *mut PageTable,
    _vaddr: usize,
    _paddr: usize,
    _size_class: u64,
    _flags: u64,
) -> Result<PreparedUserFrameMap, UserMapError> {
    Err(UserMapError::InvalidArgument)
}

pub unsafe fn commit_user_frame_map(_prepared: PreparedUserFrameMap) {}

pub unsafe fn prepare_user_page_table_map(
    _root: *mut PageTable,
    _vaddr: usize,
    _pt_kva: *mut PageTable,
) -> Result<PreparedUserPageTableMap, UserMapError> {
    Err(UserMapError::InvalidArgument)
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

pub unsafe fn reclaim_user_page_tables(root: *mut PageTable) {
    if root.is_null() {
        return;
    }
    let _guard = lock_vspace(root);
    unsafe {
        (*root).entries = [Pte::NULL; 512];
    }
}

pub unsafe fn switch_satp(_satp_val: u64) {}

pub fn set_current_vspace_root() {}

pub fn user_flags(read: bool, write: bool, exec: bool) -> u64 {
    let mut f = PTE_V | PTE_U | PTE_A | PTE_D;
    if read {
        f |= PTE_R;
    }
    if write {
        f |= PTE_W;
    }
    if exec {
        f |= PTE_X;
    }
    f
}

pub fn copy_kernel_mappings_to(_pt: *mut PageTable) {}

pub fn make_boot_root_pt() -> *mut PageTable {
    alloc_pt_page()
}

pub fn satp_for(root: *mut PageTable, _asid: u64) -> u64 {
    root as u64
}

pub fn satp_from_kva(root_kva: u64, _asid: u64) -> u64 {
    root_kva
}

const _: () = {
    assert!(PAGE_SIZE == 4096);
};
