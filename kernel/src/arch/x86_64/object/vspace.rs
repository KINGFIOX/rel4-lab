use crate::abi::constants::{KERNEL_ELF_BASE, PADDR_BASE, PHYS_BASE_RAW, PPTR_BASE, PPTR_TOP};
use crate::arch::x86_64::machine::paging::{
    PTE_G, PTE_KERNEL_RWX, PTE_PRESENT, PTE_PS, PTE_USER_RW, PTE_USER_RWX, PTE_USER_RX, PTE_W,
    PageTable, Pte, pt_index,
};
use crate::arch::x86_64::machine::registers;
use crate::kernel::smp::{BklCell, BklObjectGuard};

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

type VspaceLockGuard = BklObjectGuard;

#[inline]
fn lock_vspace(_root: *const PageTable) -> VspaceLockGuard {
    BklObjectGuard::new()
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

    #[inline]
    fn base_mut(&mut self) -> *mut PageTable {
        self.pages.as_mut_ptr()
    }
}

static BOOT_PT_POOL: BklCell<BootPtPool> = BklCell::new(BootPtPool::new());

pub fn alloc_pt_page() -> *mut PageTable {
    BOOT_PT_POOL.with_mut(|pool| {
        let idx = pool.next;
        assert!(idx < BOOT_PT_POOL_PAGES, "boot PT pool exhausted");
        pool.next += 1;
        unsafe {
            let p = pool.base_mut().add(idx);
            (*p).entries = [Pte::NULL; 512];
            p
        }
    })
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

pub unsafe fn switch_cr3(cr3: u64) {
    registers::write_cr3(cr3 as usize);
}

pub fn current_cr3() -> u64 {
    registers::read_cr3() as u64
}

pub fn set_current_vspace_root() {
    if let Some(kernel_root) = crate::kernel::smp::kernel_vspace_root() {
        if current_cr3() != kernel_root {
            unsafe { switch_cr3(kernel_root) };
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

fn kva_to_page_table_paddr(kva: usize) -> Option<usize> {
    if kva >= PPTR_BASE && kva < PPTR_TOP {
        Some(pptr_to_paddr(kva))
    } else if kva >= KERNEL_ELF_BASE {
        Some(kpptr_to_paddr(kva))
    } else {
        None
    }
}

#[inline]
fn paddr_to_user_pt_kva(paddr: usize) -> *mut PageTable {
    paddr_to_pptr(paddr) as *mut PageTable
}

fn map_1g(root: *mut PageTable, vaddr: usize, paddr: usize, flags: u64) {
    let pml4e = unsafe { &mut (*root).entries[pt_index(vaddr, 3)] };
    let pdpt = if pml4e.is_valid() {
        paddr_to_user_pt_kva(pml4e.next_pt_paddr() as usize)
    } else {
        let pdpt = alloc_pt_page();
        *pml4e = Pte::next(kpptr_to_paddr(pdpt as usize) as u64);
        pdpt
    };
    unsafe {
        (*pdpt).entries[pt_index(vaddr, 2)] = Pte::leaf(paddr as u64, flags | PTE_PS);
    }
}

pub fn copy_kernel_mappings_to(pt: *mut PageTable) {
    let _guard = lock_vspace(pt);
    let pspace_flags = PTE_PRESENT | PTE_W | PTE_G;
    for i in 0..4 {
        let pa = i * (1usize << 30);
        map_1g(pt, PPTR_BASE + pa, pa, pspace_flags);
    }
    map_1g(pt, KERNEL_ELF_BASE, PHYS_BASE_RAW, PTE_KERNEL_RWX);
}

pub fn make_boot_root_pt() -> *mut PageTable {
    let root = alloc_pt_page();
    copy_kernel_mappings_to(root);
    root
}

pub fn cr3_for(root: *mut PageTable, _asid: u64) -> u64 {
    kpptr_to_paddr(root as usize) as u64
}

pub fn cr3_from_kva(root_kva: u64, _asid: u64) -> u64 {
    kva_to_page_table_paddr(root_kva as usize).unwrap_or(0) as u64
}

pub fn vspace_root_for(root: *mut PageTable, asid: u64) -> u64 {
    cr3_for(root, asid)
}

pub unsafe fn switch_vspace(root: u64) {
    unsafe { switch_cr3(root) }
}
