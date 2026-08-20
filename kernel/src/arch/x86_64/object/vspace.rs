use crate::abi::constants::{
    KERNEL_ELF_BASE, PADDR_BASE, PHYS_BASE_RAW, PPTR_BASE, PPTR_TOP, PT_INDEX_BITS,
};
use crate::arch::x86_64::machine::paging::{
    PAGE_SHIFT, PTE_A, PTE_D, PTE_G, PTE_KERNEL_RWX, PTE_PRESENT, PTE_PS, PTE_U, PTE_USER_RW,
    PTE_USER_RWX, PTE_USER_RX, PTE_W, PageTable, Pte, ROOT_LEVEL, pt_index,
};
use crate::arch::x86_64::machine::registers;
use crate::kernel::smp::{BklCell, BklObjectGuard};
use crate::ktypes::addr::{Kva, Paddr};

pub const USER_ROOT_ENTRIES: usize = 256;
pub const USER_TOP: usize = USER_ROOT_ENTRIES << (PAGE_SHIFT + PT_INDEX_BITS * ROOT_LEVEL);

#[inline]
pub const fn pptr_to_paddr(kva: Kva) -> Paddr {
    Paddr::new(kva.raw() - PPTR_BASE + PADDR_BASE)
}

#[inline]
pub const fn paddr_to_pptr(pa: Paddr) -> Kva {
    Kva::new(pa.raw() + PPTR_BASE - PADDR_BASE)
}

#[inline]
pub const fn paddr_to_mmio(pa: Paddr) -> Kva {
    paddr_to_pptr(pa)
}

#[inline]
pub const fn kpptr_to_paddr(kva: Kva) -> Paddr {
    if kva.raw() >= KERNEL_ELF_BASE {
        Paddr::new(kva.raw() - KERNEL_ELF_BASE + PHYS_BASE_RAW)
    } else {
        pptr_to_paddr(kva)
    }
}

#[inline]
pub const fn paddr_to_kpptr(pa: Paddr) -> Kva {
    Kva::new(pa.raw() + KERNEL_ELF_BASE - PHYS_BASE_RAW)
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
        // SAFETY: operates on kernel-owned page tables from the boot pool,
        // which live for the whole run.
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
        Some(pptr_to_paddr(Kva::new(kva)).raw())
    } else if kva >= KERNEL_ELF_BASE {
        Some(kpptr_to_paddr(Kva::new(kva)).raw())
    } else {
        None
    }
}

#[inline]
fn paddr_to_user_pt_kva(paddr: usize) -> *mut PageTable {
    paddr_to_pptr(Paddr::new(paddr)).as_ptr()
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

/// Walk to the leaf slot for `vaddr`.
///
/// # Safety
/// `root` must be a live user root page table, and the intermediate tables it
/// points at must be live too, which holds while the VSpace's `PageTable` caps
/// exist.
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
        // SAFETY: the page tables reached here are the ones this function's
        // caller vouched for.
        let slot = unsafe { &mut (*pt).entries[pt_index(vaddr, level)] as *mut Pte };
        let entry = unsafe { *slot };
        if !entry.is_valid() || entry.is_leaf() {
            return Ok(PtSlotLookup { slot, bits_left });
        }
        pt = paddr_to_user_pt_kva(entry.next_pt_paddr() as usize);
        bits_left -= PT_INDEX_BITS;
    }

    // SAFETY: as above: entries of the page tables this function's caller
    // vouched for.
    let slot = unsafe { &mut (*pt).entries[pt_index(vaddr, 0)] as *mut Pte };
    Ok(PtSlotLookup { slot, bits_left })
}

/// Work out the PTE a frame mapping would install, without installing it.
///
/// # Safety
/// `root` must be a live user root page table.
pub unsafe fn prepare_user_frame_map(
    root: *mut PageTable,
    vaddr: usize,
    paddr: usize,
    size_class: u64,
    flags: u64,
) -> Result<PreparedUserFrameMap, UserMapError> {
    // SAFETY: the page tables reached here are the ones this function's
    // caller vouched for.
    unsafe { prepare_user_frame_map_at(root, vaddr, paddr, size_class, flags, false) }
}

/// As `prepare_user_frame_map`, for a mapping that already exists.
///
/// # Safety
/// `root` must be a live user root page table.
pub unsafe fn prepare_user_frame_remap(
    root: *mut PageTable,
    vaddr: usize,
    paddr: usize,
    size_class: u64,
    flags: u64,
) -> Result<PreparedUserFrameMap, UserMapError> {
    // SAFETY: the page tables reached here are the ones this function's
    // caller vouched for.
    unsafe { prepare_user_frame_map_at(root, vaddr, paddr, size_class, flags, true) }
}

/// Shared body of the frame map and remap paths.
///
/// # Safety
/// `root` must be a live user root page table.
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
    // SAFETY: the page tables reached here are the ones this function's
    // caller vouched for.
    let lookup = unsafe { lookup_pt_slot_user(root, vaddr)? };
    if lookup.bits_left != bits {
        return Err(UserMapError::FailedLookup(lookup.bits_left));
    }
    // SAFETY: as above: entries of the page tables this function's caller
    // vouched for.
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

/// Install a mapping prepared earlier.
///
/// # Safety
/// `prepared` must come from a `prepare_user_frame_map`/`_remap` call whose
/// page tables are still live and unmodified since.
pub unsafe fn commit_user_frame_map(prepared: PreparedUserFrameMap) {
    // SAFETY: the page tables reached here are the ones this function's
    // caller vouched for.
    unsafe {
        *prepared.slot = prepared.pte;
    }
    flush_vaddr_for_root(prepared.root, prepared.vaddr);
}

/// Work out where a `PageTable` cap would be linked into a VSpace.
///
/// # Safety
/// `root` and `pt` must both be live page tables.
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
    // SAFETY: the page tables reached here are the ones this function's
    // caller vouched for.
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

/// Link in a page table prepared earlier.
///
/// # Safety
/// `prepared` must come from `prepare_user_page_table_map` and its tables must
/// still be live and unmodified since.
pub unsafe fn commit_user_page_table_map(prepared: PreparedUserPageTableMap) {
    // SAFETY: the page tables reached here are the ones this function's
    // caller vouched for.
    unsafe {
        *prepared.slot = prepared.pte;
    }
    flush_vaddr_for_root(prepared.root, prepared.mapped_addr);
}

/// Unlink a page table from its VSpace.
///
/// # Safety
/// `root` and `pt` must be live page tables.
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
        // SAFETY: the page tables reached here are the ones this function's
        // caller vouched for.
        let slot = unsafe { &mut (*pt).entries[pt_index(vaddr, level)] as *mut Pte };
        let entry = unsafe { *slot };
        if !entry.is_valid() || entry.is_leaf() {
            return false;
        }
        let next_pt = paddr_to_user_pt_kva(entry.next_pt_paddr() as usize);
        if next_pt == target {
            // SAFETY: as above: entries of the page tables this function's caller
            // vouched for.
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

/// Clear a frame's mapping from its VSpace.
///
/// # Safety
/// `root` must be a live user root page table.
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
    // SAFETY: the page tables reached here are the ones this function's
    // caller vouched for.
    let lookup = unsafe { lookup_pt_slot_user(root, vaddr).ok()? };
    if lookup.bits_left != bits {
        return None;
    }
    // SAFETY: as above: entries of the page tables this function's caller
    // vouched for.
    let entry = unsafe { *lookup.slot };
    let is_mapping = entry.is_valid() && (bits == PAGE_SHIFT || entry.is_leaf());
    if !is_mapping {
        return None;
    }
    let pa = entry.leaf_pa() as usize;
    if pa != expected_pa {
        return None;
    }
    // SAFETY: the page tables reached here are the ones this function's
    // caller vouched for.
    unsafe {
        *lookup.slot = Pte::NULL;
    }
    flush_vaddr_for_root(root, vaddr);
    Some(pa)
}

/// Drop every user mapping in a VSpace, on VSpace teardown.
///
/// # Safety
/// `root` must be a live root page table that is about to be destroyed, with
/// no other core still translating through it.
pub unsafe fn reclaim_user_page_tables(root: *mut PageTable) {
    if root.is_null() {
        return;
    }
    let _guard = lock_vspace(root);
    for i in 0..USER_ROOT_ENTRIES {
        // SAFETY: the page tables reached here are the ones this function's
        // caller vouched for.
        let entry = unsafe { (*root).entries[i] };
        if !entry.is_valid() {
            continue;
        }
        if !entry.is_leaf() {
            let child = paddr_to_user_pt_kva(entry.next_pt_paddr() as usize);
            // SAFETY: as above: entries of the page tables this function's caller
            // vouched for.
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

/// Recursive helper for `reclaim_user_page_tables`.
///
/// # Safety
/// `pt` must be a live page table at `level` in the VSpace being torn down.
unsafe fn reclaim_page_table_locked(pt: *mut PageTable, level: usize) {
    for i in 0..512 {
        // SAFETY: the page tables reached here are the ones this function's
        // caller vouched for.
        let entry = unsafe { (*pt).entries[i] };
        if !entry.is_valid() {
            continue;
        }
        if !entry.is_leaf() && level > 0 {
            let child = paddr_to_user_pt_kva(entry.next_pt_paddr() as usize);
            // SAFETY: as above: entries of the page tables this function's caller
            // vouched for.
            unsafe {
                reclaim_page_table_locked(child, level - 1);
            }
        }
        unsafe {
            (*pt).entries[i] = Pte::NULL;
        }
    }
}

/// Install a new `cr3`.
///
/// # Safety
/// `cr3` must name a page table that maps the kernel image and the PSpace
/// window, since execution continues in it.
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
        // SAFETY: operates on kernel-owned page tables from the boot pool,
        // which live for the whole run.
        unsafe { switch_cr3(kernel_root) };
    }
}

fn try_switch_to_tcb_root(tcb: crate::object::tcb::TcbRef) -> bool {
    use crate::object::cap::CapTag;

    let vroot = tcb.vspace_cap();
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
        // SAFETY: operates on kernel-owned page tables from the boot pool,
        // which live for the whole run.
        unsafe { switch_cr3(new_cr3) };
    }
    true
}

pub fn set_current_vspace_root() {
    if !crate::object::tcb::current().is_some_and(try_switch_to_tcb_root) {
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
    // SAFETY: operates on kernel-owned page tables from the boot pool,
    // which live for the whole run.
    let pml4e = unsafe { &mut (*root).entries[pt_index(vaddr, 3)] };
    let pdpt = if pml4e.is_valid() {
        paddr_to_kpptr(Paddr::from_u64(pml4e.next_pt_paddr())).as_ptr()
    } else {
        let pdpt = alloc_pt_page();
        *pml4e = Pte::next(kpptr_to_paddr(Kva::new(pdpt as usize)).as_u64());
        pdpt
    };
    // SAFETY: as above: entries of the page tables this function's caller
    // vouched for.
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
    kpptr_to_paddr(Kva::new(root as usize)).as_u64()
}

pub fn cr3_from_kva(root_kva: u64, _asid: u64) -> u64 {
    kva_to_page_table_paddr(root_kva as usize).unwrap_or(0) as u64
}

pub fn vspace_root_for(root: *mut PageTable, asid: u64) -> u64 {
    cr3_for(root, asid)
}

/// Install a VSpace root as the running address space.
///
/// # Safety
/// `root` must name a page table that maps the kernel image and the PSpace
/// window, since execution continues in it.
pub unsafe fn switch_vspace(root: u64) {
    // SAFETY: the page tables reached here are the ones this function's
    // caller vouched for.
    unsafe { switch_cr3(root) }
}
