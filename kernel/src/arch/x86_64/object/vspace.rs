use crate::abi::constants::{
    KERNEL_ELF_BASE, PADDR_BASE, PHYS_BASE_RAW, PPTR_BASE, PPTR_TOP, PT_INDEX_BITS,
};
use crate::arch::x86_64::machine::paging::{
    PAGE_SHIFT, PTE_A, PTE_D, PTE_G, PTE_KERNEL_RWX, PTE_PRESENT, PTE_PS, PTE_U, PTE_USER_RW,
    PTE_USER_RWX, PTE_USER_RX, PTE_W, PageTable, Pte, ROOT_LEVEL, pt_index,
};
use crate::arch::x86_64::machine::registers;
use crate::kernel::smp::{BklCell, BklObjectGuard};

pub const USER_ROOT_ENTRIES: usize = 256;
pub const USER_TOP: usize = USER_ROOT_ENTRIES << (PAGE_SHIFT + PT_INDEX_BITS * ROOT_LEVEL);

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

struct PtSlotLookup {
    slot: *mut Pte,
    bits_left: usize,
}

pub struct PreparedUserFrameMap {
    root: *const PageTable,
    slot: *mut Pte,
    pte: Pte,
    vaddr: usize,
}

pub struct PreparedUserPageTableMap {
    root: *const PageTable,
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
fn root_paddr(root: *const PageTable) -> Option<u64> {
    if root.is_null() {
        return None;
    }
    kva_to_page_table_paddr(root as usize).map(|paddr| paddr as u64)
}

#[inline]
fn cr3_root_paddr(cr3: u64) -> u64 {
    cr3 & 0x000f_ffff_ffff_f000
}

#[inline]
fn flush_vaddr_for_root(root: *const PageTable, vaddr: usize) {
    match root_paddr(root) {
        Some(root_pa) if cr3_root_paddr(current_cr3()) == root_pa => {
            crate::arch::x86_64::machine::tlb_flush_vaddr(vaddr);
        }
        _ => {
            crate::arch::x86_64::machine::tlb_flush_all();
        }
    }
    crate::kernel::smp::remote_tlb_flush_all();
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
    let mut bits_left = PAGE_SHIFT + PT_INDEX_BITS * ROOT_LEVEL;
    for level in (1..=ROOT_LEVEL).rev() {
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
    flags |= PTE_PRESENT | PTE_U | PTE_A | PTE_D;
    if bits > PAGE_SHIFT {
        flags |= PTE_PS;
    }

    let _guard = lock_vspace(root);
    let lookup = unsafe { lookup_pt_slot_user(root, vaddr)? };
    if lookup.bits_left != bits {
        return Err(UserMapError::FailedLookup(lookup.bits_left));
    }
    let entry = unsafe { *lookup.slot };
    if entry.is_valid() {
        let existing_leaf = bits == PAGE_SHIFT || entry.is_leaf();
        if !existing_leaf || !replace_existing_leaf {
            return Err(UserMapError::DeleteFirst);
        }
    }
    Ok(PreparedUserFrameMap {
        root,
        slot: lookup.slot,
        pte: Pte::leaf(paddr as u64, flags),
        vaddr,
    })
}

pub unsafe fn commit_user_frame_map(prepared: PreparedUserFrameMap) {
    unsafe {
        *prepared.slot = prepared.pte;
    }
    flush_vaddr_for_root(prepared.root, prepared.vaddr);
}

pub unsafe fn prepare_user_page_table_map(
    root: *mut PageTable,
    vaddr: usize,
    pt_kva: *mut PageTable,
    expected_coverage: Option<usize>,
) -> Result<PreparedUserPageTableMap, UserMapError> {
    if root.is_null() || pt_kva.is_null() || vaddr >= USER_TOP {
        return Err(UserMapError::InvalidArgument);
    }
    // seL4 masks the syscall vaddr to the table window; libsel4vspace
    // passes the page address, which is only 4K-aligned.
    let vaddr = if let Some(bits) = expected_coverage.filter(|bits| *bits != 0) {
        let aligned = vaddr & !((1usize << bits) - 1);
        if !user_range_aligned(aligned, bits) {
            return Err(UserMapError::InvalidArgument);
        }
        aligned
    } else {
        vaddr
    };
    let pt_pa = kva_to_page_table_paddr(pt_kva as usize).ok_or(UserMapError::InvalidArgument)?;

    let _guard = lock_vspace(root);
    let lookup = unsafe { lookup_pt_slot_user(root, vaddr)? };
    let entry = unsafe { *lookup.slot };
    if let Some(bits) = expected_coverage.filter(|bits| *bits != 0) {
        if lookup.bits_left != bits {
            return Err(UserMapError::FailedLookup(lookup.bits_left));
        }
    } else if lookup.bits_left == PAGE_SHIFT {
        return Err(UserMapError::DeleteFirst);
    }
    if entry.is_valid() {
        return Err(UserMapError::DeleteFirst);
    }

    let mapped_addr = vaddr & !((1usize << lookup.bits_left) - 1);
    Ok(PreparedUserPageTableMap {
        root,
        slot: lookup.slot,
        pte: Pte::next(pt_pa as u64),
        mapped_addr,
    })
}

pub unsafe fn commit_user_page_table_map(prepared: PreparedUserPageTableMap) {
    unsafe {
        *prepared.slot = prepared.pte;
    }
    flush_vaddr_for_root(prepared.root, prepared.mapped_addr);
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
    for level in (1..=ROOT_LEVEL).rev() {
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
            flush_vaddr_for_root(root, vaddr);
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
    let is_mapping = entry.is_valid() && (bits == PAGE_SHIFT || entry.is_leaf());
    if !is_mapping {
        return None;
    }
    let pa = entry.leaf_pa() as usize;
    if pa != expected_pa {
        return None;
    }
    unsafe {
        *lookup.slot = Pte::NULL;
    }
    flush_vaddr_for_root(root, vaddr);
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
                reclaim_page_table_locked(child, ROOT_LEVEL - 1);
            }
        }
        unsafe {
            (*root).entries[i] = Pte::NULL;
        }
    }
    crate::arch::x86_64::machine::tlb_flush_all();
    crate::kernel::smp::remote_tlb_flush_all();
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

pub unsafe fn switch_cr3(cr3: u64) {
    registers::write_cr3(cr3 as usize);
}

pub fn current_cr3() -> u64 {
    registers::read_cr3() as u64
}

fn switch_to_kernel_root() {
    let Some(kernel_root) = crate::kernel::smp::kernel_vspace_root() else {
        return;
    };
    if current_cr3() != kernel_root {
        unsafe { switch_cr3(kernel_root) };
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
    let new_cr3 = cr3_from_kva(root_kva, asid as u64);
    if new_cr3 == 0 {
        return false;
    }
    if current_cr3() != new_cr3 {
        unsafe { switch_cr3(new_cr3) };
    }
    true
}

pub fn set_current_vspace_root() {
    let current = crate::object::tcb::current();
    if !try_switch_to_tcb_root(current) {
        switch_to_kernel_root();
    }
}

pub fn user_flags(read: bool, write: bool, exec: bool) -> u64 {
    user_frame_flags(read, write, exec, false)
}

pub fn user_frame_flags(read: bool, write: bool, exec: bool, _is_device: bool) -> u64 {
    let flags = match (read, write, exec) {
        (true, true, true) => PTE_USER_RWX,
        (true, true, false) => PTE_USER_RW,
        (true, false, true) => PTE_USER_RX,
        _ => 0,
    };
    if flags == 0 { 0 } else { flags | PTE_A | PTE_D }
}

fn map_1g(root: *mut PageTable, vaddr: usize, paddr: usize, flags: u64) {
    let vaddr = vaddr & !((1usize << 30) - 1);
    let paddr = paddr & !((1usize << 30) - 1);
    let pml4e = unsafe { &mut (*root).entries[pt_index(vaddr, 3)] };
    let pdpt = if pml4e.is_valid() {
        paddr_to_kpptr(pml4e.next_pt_paddr() as usize) as *mut PageTable
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
