//! LoongArch64 VSpace backend staging.
//!
//! User paging objects follow the same seL4-style explicit page-table model as
//! the RISC-V backend. The current LoongArch bring-up still leaves hardware
//! root switching as a no-op, but frame and page-table caps now mutate real
//! kernel-side page-table objects instead of rejecting every mapping request.

use crate::abi::constants::{
    KERNEL_ELF_BASE, PADDR_BASE, PHYS_BASE_RAW, PPTR_BASE, PPTR_TOP, PT_INDEX_BITS,
};
use crate::arch::loongarch64::paging::{
    PAGE_SHIFT, PAGE_SIZE, PTE_A, PTE_D, PTE_R, PTE_U, PTE_V, PTE_W, PTE_X, PageTable, Pte,
    pt_index,
};
use crate::kernel::smp::{BklCell, BklObjectGuard};

const USER_ROOT_ENTRIES: usize = 1 << (PT_INDEX_BITS - 1);
pub const USER_TOP: usize = USER_ROOT_ENTRIES << (PAGE_SHIFT + PT_INDEX_BITS * 2);

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

struct PtSlotLookup {
    slot: *mut Pte,
    bits_left: usize,
}

pub struct PreparedUserFrameMap {
    slot: *mut Pte,
    pte: Pte,
    vaddr: usize,
}

pub struct PreparedUserPageTableMap {
    slot: *mut Pte,
    pte: Pte,
    mapped_addr: usize,
}

impl PreparedUserPageTableMap {
    #[inline]
    pub const fn mapped_addr(&self) -> usize {
        self.mapped_addr
    }
}

#[inline]
const fn page_bits_for_size_class(size_class: u64) -> Option<usize> {
    match size_class {
        0 => Some(PAGE_SHIFT),
        1 => Some(PAGE_SHIFT + PT_INDEX_BITS),
        2 => Some(PAGE_SHIFT + PT_INDEX_BITS * 2),
        _ => None,
    }
}

#[inline]
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

#[inline]
fn user_range_aligned(vaddr: usize, bits: usize) -> bool {
    let size = 1usize << bits;
    vaddr & (size - 1) == 0
        && match vaddr.checked_add(size) {
            Some(end) => end <= USER_TOP,
            None => false,
        }
}

unsafe fn lookup_pt_slot_user(
    root: *mut PageTable,
    vaddr: usize,
) -> Result<PtSlotLookup, UserMapError> {
    if root.is_null() || vaddr >= USER_TOP {
        return Err(UserMapError::InvalidArgument);
    }

    let mut pt = root;
    let mut bits_left = PAGE_SHIFT + PT_INDEX_BITS * 2;
    for level in (1..=2).rev() {
        let slot = unsafe { &mut (*pt).entries[pt_index(vaddr, level)] as *mut Pte };
        let entry = unsafe { *slot };
        if !entry.is_valid() || entry.is_leaf() {
            return Ok(PtSlotLookup { slot, bits_left });
        }
        pt = paddr_to_user_pt_kva(entry.next_pt_paddr() as usize);
        bits_left -= PT_INDEX_BITS;
    }

    let slot = unsafe { &mut (*pt).entries[pt_index(vaddr, 0)] as *mut Pte };
    Ok(PtSlotLookup { slot, bits_left })
}

pub unsafe fn prepare_user_frame_map(
    root: *mut PageTable,
    vaddr: usize,
    paddr: usize,
    size_class: u64,
    flags: u64,
) -> Result<PreparedUserFrameMap, UserMapError> {
    unsafe { prepare_user_frame_map_at(root, vaddr, paddr, size_class, flags, false) }
}

pub unsafe fn prepare_user_frame_remap(
    root: *mut PageTable,
    vaddr: usize,
    paddr: usize,
    size_class: u64,
    flags: u64,
) -> Result<PreparedUserFrameMap, UserMapError> {
    unsafe { prepare_user_frame_map_at(root, vaddr, paddr, size_class, flags, true) }
}

unsafe fn prepare_user_frame_map_at(
    root: *mut PageTable,
    vaddr: usize,
    paddr: usize,
    size_class: u64,
    mut flags: u64,
    replace_existing_leaf: bool,
) -> Result<PreparedUserFrameMap, UserMapError> {
    let bits = page_bits_for_size_class(size_class).ok_or(UserMapError::InvalidArgument)?;
    if !user_range_aligned(vaddr, bits) || paddr & ((1usize << bits) - 1) != 0 {
        return Err(UserMapError::InvalidArgument);
    }
    flags |= PTE_U | PTE_V | PTE_A | PTE_D;

    let _guard = lock_vspace(root);
    let lookup = unsafe { lookup_pt_slot_user(root, vaddr)? };
    if lookup.bits_left != bits {
        return Err(UserMapError::FailedLookup(lookup.bits_left));
    }
    let entry = unsafe { *lookup.slot };
    if entry.is_valid() {
        if !entry.is_leaf() || !replace_existing_leaf {
            return Err(UserMapError::DeleteFirst);
        }
    }
    Ok(PreparedUserFrameMap {
        slot: lookup.slot,
        pte: Pte::leaf(paddr as u64, flags),
        vaddr,
    })
}

pub unsafe fn commit_user_frame_map(prepared: PreparedUserFrameMap) {
    unsafe {
        *prepared.slot = prepared.pte;
    }
    crate::arch::loongarch64::csr::sfence_vma_va(prepared.vaddr);
    crate::kernel::smp::remote_sfence_vma_all();
}

pub unsafe fn prepare_user_page_table_map(
    root: *mut PageTable,
    vaddr: usize,
    pt_kva: *mut PageTable,
) -> Result<PreparedUserPageTableMap, UserMapError> {
    if root.is_null() || pt_kva.is_null() || vaddr >= USER_TOP {
        return Err(UserMapError::InvalidArgument);
    }
    let pt_pa = kva_to_page_table_paddr(pt_kva as usize).ok_or(UserMapError::InvalidArgument)?;

    let _guard = lock_vspace(root);
    let lookup = unsafe { lookup_pt_slot_user(root, vaddr)? };
    let entry = unsafe { *lookup.slot };
    if lookup.bits_left == PAGE_SHIFT || entry.is_valid() {
        return Err(UserMapError::DeleteFirst);
    }

    let mapped_addr = vaddr & !((1usize << lookup.bits_left) - 1);
    Ok(PreparedUserPageTableMap {
        slot: lookup.slot,
        pte: Pte::next(pt_pa as u64),
        mapped_addr,
    })
}

pub unsafe fn commit_user_page_table_map(prepared: PreparedUserPageTableMap) {
    unsafe {
        *prepared.slot = prepared.pte;
    }
    crate::arch::loongarch64::csr::sfence_vma_va(prepared.mapped_addr);
    crate::kernel::smp::remote_sfence_vma_all();
}

pub unsafe fn unmap_user_page_table(
    root: *mut PageTable,
    vaddr: usize,
    target: *mut PageTable,
) -> bool {
    if root.is_null() || target.is_null() || root == target || vaddr >= USER_TOP {
        return false;
    }

    let _guard = lock_vspace(root);
    let mut pt = root;
    for level in (1..=2).rev() {
        let slot = unsafe { &mut (*pt).entries[pt_index(vaddr, level)] as *mut Pte };
        let entry = unsafe { *slot };
        if !entry.is_valid() || entry.is_leaf() {
            return false;
        }
        let next_pt = paddr_to_user_pt_kva(entry.next_pt_paddr() as usize);
        if next_pt == target {
            unsafe {
                *slot = Pte::NULL;
            }
            crate::arch::loongarch64::csr::sfence_vma_all();
            crate::kernel::smp::remote_sfence_vma_all();
            return true;
        }
        pt = next_pt;
    }
    false
}

pub unsafe fn unmap_user_frame(
    root: *mut PageTable,
    vaddr: usize,
    size_class: u64,
    expected_pa: usize,
) -> Option<usize> {
    let bits = match page_bits_for_size_class(size_class) {
        Some(bits) => bits,
        None => return None,
    };
    if root.is_null() || !user_range_aligned(vaddr, bits) {
        return None;
    }

    let _guard = lock_vspace(root);
    let lookup = unsafe { lookup_pt_slot_user(root, vaddr).ok()? };
    if lookup.bits_left != bits {
        return None;
    }
    let entry = unsafe { *lookup.slot };
    if !entry.is_valid() || !entry.is_leaf() {
        return None;
    }
    let pa = entry.leaf_pa() as usize;
    if pa != expected_pa {
        return None;
    }
    unsafe {
        *lookup.slot = Pte::NULL;
    }
    crate::arch::loongarch64::csr::sfence_vma_va(vaddr);
    crate::kernel::smp::remote_sfence_vma_all();
    Some(pa)
}

pub unsafe fn reclaim_user_page_tables(root: *mut PageTable) {
    if root.is_null() {
        return;
    }
    let _guard = lock_vspace(root);
    for i in 0..USER_ROOT_ENTRIES {
        let entry = unsafe { (*root).entries[i] };
        if !entry.is_valid() {
            continue;
        }
        if !entry.is_leaf() {
            let child = paddr_to_user_pt_kva(entry.next_pt_paddr() as usize);
            unsafe {
                reclaim_page_table_locked(child, 1);
            }
        }
        unsafe {
            (*root).entries[i] = Pte::NULL;
        }
    }
    crate::arch::loongarch64::csr::sfence_vma_all();
    crate::kernel::smp::remote_sfence_vma_all();
}

unsafe fn reclaim_page_table_locked(pt: *mut PageTable, level: usize) {
    for i in 0..512 {
        let entry = unsafe { (*pt).entries[i] };
        if !entry.is_valid() {
            continue;
        }
        if !entry.is_leaf() && level > 0 {
            let child = paddr_to_user_pt_kva(entry.next_pt_paddr() as usize);
            unsafe {
                reclaim_page_table_locked(child, level - 1);
            }
        }
        unsafe {
            (*pt).entries[i] = Pte::NULL;
        }
    }
}

pub unsafe fn switch_satp(_satp_val: u64) {
    crate::arch::loongarch64::csr::sfence_vma_all();
    crate::arch::loongarch64::csr::fence_i();
}

pub fn set_current_vspace_root() {
    let current = crate::object::tcb::current();
    if !try_switch_to_tcb_root(current) {
        switch_to_kernel_root();
    }
}

fn switch_to_kernel_root() {
    let Some(kernel_satp) = crate::kernel::smp::kernel_satp() else {
        return;
    };
    unsafe {
        switch_satp(kernel_satp);
    }
}

fn try_switch_to_tcb_root(tcb: *const crate::object::tcb::Tcb) -> bool {
    use crate::object::cap::CapTag;

    if tcb.is_null() {
        return false;
    }
    let vroot = crate::object::tcb::vspace_cap_snapshot(tcb);
    if vroot.tag() != Some(CapTag::PageTable) {
        return false;
    }
    let root_kva = vroot.page_table_base_ptr();
    let asid = vroot.page_table_mapped_asid();
    if root_kva == 0 || !vroot.page_table_is_mapped() || asid == 0 {
        return false;
    }
    if crate::object::asid::lookup(asid) != root_kva {
        return false;
    }
    let new_satp = satp_from_kva(root_kva, asid as u64);
    if new_satp == 0 {
        return false;
    }
    unsafe {
        switch_satp(new_satp);
    }
    true
}

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
