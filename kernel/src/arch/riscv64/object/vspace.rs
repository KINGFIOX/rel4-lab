//! Kernel + user VSpace helpers.
//!
//! The boot page-table pool is reserved for kernel-owned boot objects: the
//! initial root VSpace and the initial thread's boot-created user paging
//! structures. Runtime user mappings follow seL4's explicit paging-object
//! model: user frames can only be mapped through already-mapped `PageTable`
//! caps, and `PageTable_Map` installs those caps into the VSpace.

use crate::abi::constants::{
    KERNEL_ELF_BASE, PADDR_BASE, PHYS_BASE_RAW, PPTR_BASE, PPTR_TOP, PT_INDEX_BITS,
};
use crate::arch::riscv64::machine::csr;
use crate::arch::riscv64::machine::paging::{
    PTE_A, PTE_D, PTE_G, PTE_R, PTE_U, PTE_V, PTE_W, PTE_X, PageTable, Pte, RISCV_PG_SHIFT,
    make_satp, pt_index,
};
use crate::kernel::smp::{BklCell, BklObjectGuard};
use crate::ktypes::addr::{Kva, Paddr};

const USER_ROOT_ENTRIES: usize = 1 << (PT_INDEX_BITS - 1);
pub const USER_TOP: usize = USER_ROOT_ENTRIES << (RISCV_PG_SHIFT + PT_INDEX_BITS * 2);

type VspaceLockGuard = BklObjectGuard;

#[inline]
fn lock_vspace(_root: *const PageTable) -> VspaceLockGuard {
    BklObjectGuard::new()
}

/// Convert a kernel-window (PSpace) virtual address to its physical
/// address. The C kernel calls this `addrFromPPtr`.
///
/// Only valid for VAs in `[PPTR_BASE, PPTR_TOP)`. Boot code that runs before
/// `make_boot_root_pt` installs the PSpace mappings must use
/// `kpptr_to_paddr` / `paddr_to_kpptr` instead.
#[inline]
pub const fn pptr_to_paddr(kva: Kva) -> Paddr {
    Paddr::new(kva.raw() - PPTR_BASE + PADDR_BASE)
}

#[inline]
pub const fn paddr_to_pptr(pa: Paddr) -> Kva {
    Kva::new(pa.raw() - PADDR_BASE + PPTR_BASE)
}

#[inline]
pub const fn paddr_to_mmio(pa: Paddr) -> Kva {
    paddr_to_pptr(pa)
}

/// Convert a kernel-ELF VA (anything in the kernel image: text, rodata,
/// data, bss) to its physical address. Valid for VAs in `[KERNEL_ELF_BASE,
/// KERNEL_ELF_BASE + image_size)`. The kernel ELF window is set up by the
/// elfloader before our `_start` runs.
#[inline]
pub const fn kpptr_to_paddr(kva: Kva) -> Paddr {
    Paddr::new(kva.raw() - KERNEL_ELF_BASE + PHYS_BASE_RAW)
}

#[inline]
pub const fn paddr_to_kpptr(pa: Paddr) -> Kva {
    Kva::new(pa.raw() - PHYS_BASE_RAW + KERNEL_ELF_BASE)
}

// ---- Boot-time PT page pool ----------------------------------------------
//
// During boot we may need fresh 4 KiB page-table pages to add user-image
// mappings. We carve them out of a static pool in `.bss`. This pool is
// distinct from the rootserver-visible "untyped" memory and is only used
// by the kernel itself.

// The boot pool backs kernel-owned page-table pages: the initial root
// VSpace, the initial thread's boot-created user paging objects, and any
// kernel boot mappings. Normal user mappings are made through user-visible
// `PageTable` caps retyped from Untyped, matching seL4's explicit
// paging-object model. The initial boot-created user paging objects are
// exposed through BootInfo's `userImagePaging` range.
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

/// Allocate a fresh zeroed page-table page from the boot pool. Returns its
/// kernel-window virtual address. This is for boot-created kernel-owned
/// paging objects only; runtime user paging objects come from Untyped.
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
        0 => Some(RISCV_PG_SHIFT),
        1 => Some(RISCV_PG_SHIFT + PT_INDEX_BITS),
        2 => Some(RISCV_PG_SHIFT + PT_INDEX_BITS * 2),
        _ => None,
    }
}

#[inline]
fn kva_to_page_table_paddr(kva: usize) -> Option<usize> {
    page_table_paddr(Kva::new(kva)).map(Paddr::raw)
}

/// Physical address of a page table, picking the translation for whichever
/// kernel window its address lives in.
#[inline]
fn page_table_paddr(kva: Kva) -> Option<Paddr> {
    if kva.raw() >= PPTR_BASE && kva.raw() < PPTR_TOP {
        Some(pptr_to_paddr(kva))
    } else if kva.raw() >= KERNEL_ELF_BASE {
        Some(kpptr_to_paddr(kva))
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
fn satp_root_paddr(satp: u64) -> u64 {
    (satp & ((1u64 << 44) - 1)) << RISCV_PG_SHIFT
}

#[inline]
fn flush_vaddr_for_root(root: *const PageTable, vaddr: usize) {
    match root_paddr(root) {
        Some(root_pa) if satp_root_paddr(csr::satp() as u64) == root_pa => {
            csr::sfence_vma_va(vaddr);
        }
        _ => {
            csr::sfence_vma_all();
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
    let mut bits_left = RISCV_PG_SHIFT + PT_INDEX_BITS * 2;
    for level in (1..=2).rev() {
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

/// Prepare a RISC-V frame-cap mapping at its natural Sv39 level:
///
/// * size class 0: 4 KiB leaf at level 0
/// * size class 1: 2 MiB leaf at level 1
/// * size class 2: 1 GiB leaf at level 2
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
    flags |= PTE_U | PTE_V | PTE_A | PTE_D;

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
        if !entry.is_leaf() || !replace_existing_leaf {
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
    } else if lookup.bits_left == RISCV_PG_SHIFT {
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
    for level in (1..=2).rev() {
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

/// Remove a user frame mapping at the natural Sv39 level for the cap's
/// size class. This does not reclaim interior page-table objects: mapped
/// user `PageTable` caps manage those pages explicitly.
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
    flush_vaddr_for_root(root, vaddr);
    Some(pa)
}

/// Clear the user half of a root page table recursively.
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
                reclaim_page_table_locked(child, 1);
            }
        }
        unsafe {
            (*root).entries[i] = Pte::NULL;
        }
    }
    csr::sfence_vma_all();
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

/// Install a fresh `satp` value, then flush the TLB.
/// Install a new `satp`, flushing translations around the change.
///
/// # Safety
/// `satp_val` must name a page table that maps the kernel image and the PSpace
/// window, since execution continues in it.
pub unsafe fn switch_satp(satp_val: u64) {
    csr::sfence_vma_all();
    csr::set_satp(satp_val as usize);
    csr::sfence_vma_all();
    csr::fence_i();
}

fn switch_to_kernel_root() {
    let Some(kernel_satp) = crate::kernel::smp::kernel_vspace_root() else {
        return;
    };
    if csr::satp() as u64 != kernel_satp {
        // SAFETY: operates on kernel-owned page tables from the boot pool,
        // which live for the whole run.
        unsafe { switch_satp(kernel_satp) };
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
    let new_satp = satp_from_kva(root_kva, asid as u64);
    if new_satp == 0 {
        return false;
    }
    if csr::satp() as u64 != new_satp {
        // SAFETY: operates on kernel-owned page tables from the boot pool,
        // which live for the whole run.
        unsafe { switch_satp(new_satp) };
    }
    true
}

/// Mirror RISC-V seL4 `setVMRoot(ksCurThread)`: after ASID deletion,
/// re-evaluate the current TCB's VSpace and fall back to the kernel root
/// if its page-table cap no longer resolves through the ASID table.
pub fn set_current_vspace_root() {
    if !crate::object::tcb::current().is_some_and(try_switch_to_tcb_root) {
        switch_to_kernel_root();
    }
}

/// `seL4` user permissions ⇒ Sv39 PTE flag bits (for U-mode, 4K page).
pub fn user_flags(read: bool, write: bool, exec: bool) -> u64 {
    user_frame_flags(read, write, exec, false)
}

/// `seL4` user permissions ⇒ Sv39 PTE flag bits for frame caps.
///
/// RISC-V Sv39 does not encode a device/cacheability attribute in the leaf PTE,
/// so device frames use the same flags as regular frames.
pub fn user_frame_flags(read: bool, write: bool, exec: bool, _is_device: bool) -> u64 {
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

/// Populate the kernel & PSpace L2 entries on a freshly-zeroed root PT.
///
/// User PTs (allocated by the rootserver via `Untyped_Retype` →
/// `PageTable`) come out of Untyped fully zeroed, so a `satp` swap
/// to them would leave the kernel window untranslatable — the very
/// next trap from U-mode would fetch from VA `trap_entry` ∈ the
/// kernel ELF window, fault, and re-trap forever. Mirrors the
/// `copyGlobalMappings` step in `Arch_initPageTable` /
/// `kernel/src/object/structures.bf` derivatives.
pub fn copy_kernel_mappings_to(pt: *mut PageTable) {
    use crate::abi::constants::{KERNEL_ELF_BASE, PPTR_BASE};
    let kernel_flags = PTE_V | PTE_R | PTE_W | PTE_X | PTE_G | PTE_A | PTE_D;
    let pspace_flags = PTE_V | PTE_R | PTE_W | PTE_G | PTE_A | PTE_D;
    let _guard = lock_vspace(pt);

    let kernel_l2_index = pt_index(KERNEL_ELF_BASE, 2);
    let kernel_pa = 0x8000_0000u64;
    // SAFETY: operates on kernel-owned page tables from the boot pool,
    // which live for the whole run.
    unsafe {
        (*pt).entries[kernel_l2_index] = Pte::leaf(kernel_pa, kernel_flags);
    }
    let pspace_base_l2 = pt_index(PPTR_BASE, 2);
    for i in 0..8 {
        let pa = (i as u64) * (1u64 << 30);
        // SAFETY: as above: entries of the page tables this function's caller
        // vouched for.
        unsafe {
            (*pt).entries[pspace_base_l2 + i] = Pte::leaf(pa, pspace_flags);
        }
    }
}

/// Build a fresh root Sv39 page table with kernel + PSpace mappings:
///
///   • Kernel ELF window at L2[510] (single 1 GiB megapage,
///     VA `KERNEL_ELF_BASE` → PA 0x8000_0000, R/W/X kernel-only).
///   • PSpace window covering PA [0, 4 GiB) via L2[256..260] (four
///     1 GiB megapages, R/W kernel-only). The PSpace VA for PA `p` is
///     `PPTR_BASE + p`; we use it as the `capPtr` encoding for both
///     regular and device untyped/frame caps.
///
/// Initial user-image mappings are installed during boot and exposed as
/// BootInfo `userImagePaging` caps; later user mappings require explicit
/// `PageTable` caps.
pub fn make_boot_root_pt() -> *mut PageTable {
    let root = alloc_pt_page();
    let kernel_flags = PTE_V | PTE_R | PTE_W | PTE_X | PTE_G | PTE_A | PTE_D;
    let pspace_flags = PTE_V | PTE_R | PTE_W | PTE_G | PTE_A | PTE_D;
    let _guard = lock_vspace(root);

    let kernel_l2_index = pt_index(KERNEL_ELF_BASE, 2);
    let kernel_pa = 0x8000_0000u64;
    // SAFETY: operates on kernel-owned page tables from the boot pool,
    // which live for the whole run.
    unsafe {
        (*root).entries[kernel_l2_index] = Pte::leaf(kernel_pa, kernel_flags);
    }

    // PSpace: map PA [0, 8 GiB) at PSpace VAs 0xFFFFFFC0_00000000 ..
    // 0xFFFFFFC2_00000000 (i.e. L2[256..264]). Eight 1 GiB megapages
    // gives us comfortable headroom over QEMU virt's typical 3–4 GiB
    // RAM range while still using only one extra 8-byte PTE per GiB.
    let pspace_base_l2 = pt_index(crate::abi::constants::PPTR_BASE, 2);
    for i in 0..8 {
        let pa = (i as u64) * (1u64 << 30);
        // SAFETY: operates on kernel-owned page tables from the boot pool,
        // which live for the whole run.
        unsafe {
            (*root).entries[pspace_base_l2 + i] = Pte::leaf(pa, pspace_flags);
        }
    }
    root
}

/// Compose a Sv39 `satp` value for the given root PT (kernel-ELF VA) and
/// ASID, by translating its VA to its physical address.
pub fn satp_for(root: *mut PageTable, asid: u64) -> u64 {
    let pa = kpptr_to_paddr(Kva::new(root as usize));
    make_satp(asid, pa.as_u64())
}

/// Compose a Sv39 `satp` from a root PT KVA, picking the right physical
/// translation based on which kernel window the KVA lives in:
///
///   * `PPTR_BASE .. PPTR_TOP`            → PSpace direct map (user PTs
///                                           allocated from Untyped).
///   * `KERNEL_ELF_BASE .. PPTR_BASE+...` → kernel ELF / `.bss` window
///                                           (the boot root PT only).
///
/// Returns 0 for KVAs outside both windows so callers can ignore them
/// instead of programming a bogus `satp`.
pub fn satp_from_kva(root_kva: u64, asid: u64) -> u64 {
    match page_table_paddr(Kva::from_u64(root_kva)) {
        Some(pa) => make_satp(asid, pa.as_u64()),
        None => 0,
    }
}

pub fn vspace_root_for(root: *mut PageTable, asid: u64) -> u64 {
    satp_for(root, asid)
}

/// Install a VSpace root as the running address space.
///
/// # Safety
/// `root` must name a page table that maps the kernel image and the PSpace
/// window, since execution continues in it.
pub unsafe fn switch_vspace(root: u64) {
    // SAFETY: the page tables reached here are the ones this function's
    // caller vouched for.
    unsafe { switch_satp(root) }
}
