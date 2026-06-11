//! Kernel + user VSpace helpers.
//!
//! For M2 we keep things very simple: the elfloader hands us a Sv39 root PT
//! already containing the 1 GiB kernel-window mapping. We simply walk that
//! page table to install user-image mappings, allocating fresh 4 KiB PT
//! pages out of a static kernel boot pool when needed.

use crate::abi::constants::{
    KERNEL_ELF_BASE, PADDR_BASE, PHYS_BASE_RAW, PPTR_BASE, PPTR_TOP, PT_INDEX_BITS, RISCV_PG_SHIFT,
};
use crate::arch::riscv64::csr;
use crate::arch::riscv64::sv39::{
    PAGE_SIZE, PTE_A, PTE_D, PTE_G, PTE_R, PTE_U, PTE_V, PTE_W, PTE_X, PageTable, Pte, make_satp,
    pt_index,
};
use crate::kernel::smp::{BklCell, BklObjectGuard};

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
/// Only valid for VAs in `[PPTR_BASE, PPTR_TOP)`. **The PSpace window is
/// not mapped by the elfloader**; it gets set up by us later in M3+. For
/// boot-time use the `kpptr_to_paddr` / `paddr_to_kpptr` helpers below.
#[inline]
#[allow(dead_code)]
pub const fn pptr_to_paddr(vaddr: usize) -> usize {
    vaddr - PPTR_BASE + PADDR_BASE
}

#[inline]
#[allow(dead_code)]
pub const fn paddr_to_pptr(paddr: usize) -> usize {
    paddr - PADDR_BASE + PPTR_BASE
}

#[inline]
#[allow(dead_code)]
pub const fn is_kernel_vaddr(va: usize) -> bool {
    va >= PPTR_BASE && va < PPTR_TOP
}

/// Convert a kernel-ELF VA (anything in the kernel image: text, rodata,
/// data, bss) to its physical address. Valid for VAs in `[KERNEL_ELF_BASE,
/// KERNEL_ELF_BASE + image_size)`. The kernel ELF window is set up by the
/// elfloader before our `_start` runs.
#[inline]
pub const fn kpptr_to_paddr(vaddr: usize) -> usize {
    vaddr - KERNEL_ELF_BASE + PHYS_BASE_RAW
}

#[inline]
pub const fn paddr_to_kpptr(paddr: usize) -> usize {
    paddr - PHYS_BASE_RAW + KERNEL_ELF_BASE
}

// ---- Boot-time PT page pool ----------------------------------------------
//
// During boot we may need fresh 4 KiB page-table pages to add user-image
// mappings. We carve them out of a static pool in `.bss`. This pool is
// distinct from the rootserver-visible "untyped" memory and is only used
// by the kernel itself.

// The boot pool backs kernel-owned page-table pages: the initial root
// VSpace, the initial thread's boot-created user paging objects, and any
// legacy kernel boot mappings. Normal user mappings are made through
// user-visible `PageTable` caps retyped from Untyped, matching seL4's
// explicit paging-object model. The initial boot-created user paging
// objects are also exposed through BootInfo's `userImagePaging` range.
const BOOT_PT_POOL_PAGES: usize = 1024;
const BOOT_PT_FREELIST_EMPTY: usize = usize::MAX;

#[repr(C, align(4096))]
struct BootPtPool {
    pages: [PageTable; BOOT_PT_POOL_PAGES],
    next: usize,
    freelist_head: usize,
}

impl BootPtPool {
    const fn new() -> Self {
        Self {
            pages: [const { PageTable::zeroed() }; BOOT_PT_POOL_PAGES],
            next: 0,
            freelist_head: BOOT_PT_FREELIST_EMPTY,
        }
    }

    #[inline]
    fn base(&self) -> *const PageTable {
        self.pages.as_ptr()
    }

    #[inline]
    fn base_mut(&mut self) -> *mut PageTable {
        self.pages.as_mut_ptr()
    }

    #[inline]
    fn contains(&self, p: *mut PageTable) -> bool {
        let base = self.base() as usize;
        let end = base + BOOT_PT_POOL_PAGES * core::mem::size_of::<PageTable>();
        let v = p as usize;
        v >= base && v < end && (v - base) % core::mem::size_of::<PageTable>() == 0
    }

    #[inline]
    fn index_of(&self, p: *mut PageTable) -> usize {
        ((p as usize) - (self.base() as usize)) / core::mem::size_of::<PageTable>()
    }
}

static BOOT_PT_POOL: BklCell<BootPtPool> = BklCell::new(BootPtPool::new());

/// Allocate a fresh zeroed page-table page from the boot pool. Returns its
/// kernel-window virtual address. Prefers the freelist over the bump
/// pointer so long-running suites don't starve the pool.
pub fn alloc_pt_page() -> *mut PageTable {
    BOOT_PT_POOL.with_mut(|pool| {
        let head = pool.freelist_head;
        if head != BOOT_PT_FREELIST_EMPTY {
            unsafe {
                let p = pool.base_mut().add(head);
                // Next-free index is stashed in entries[0].
                let next = (*p).entries[0].raw() as usize;
                pool.freelist_head = next;
                (*p).entries = [Pte::NULL; 512];
                return p;
            }
        }
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

/// Push a PT page back onto the boot-pool freelist. Silently ignores
/// pages outside the pool (e.g. caller-owned page-table objects we
/// never allocated).
pub unsafe fn free_pt_page(p: *mut PageTable) {
    BOOT_PT_POOL.with_mut(|pool| {
        if !pool.contains(p) {
            return;
        }
        let idx = pool.index_of(p);
        unsafe {
            let head = pool.freelist_head;
            (*p).entries[0] = Pte::from_raw(head as u64);
            // Zero the rest so a stale entry can't accidentally look valid.
            for i in 1..512 {
                (*p).entries[i] = Pte::NULL;
            }
            pool.freelist_head = idx;
        }
    });
}

/// Read the currently active Sv39 root PT from `satp`. The elfloader places
/// its boot PT inside its own image (low PA region) which is _not_ in our
/// kernel ELF window nor in the PSpace window, so this returns the raw
/// physical address — callers that want to read it must arrange a mapping.
#[allow(dead_code)]
pub fn current_root_pt_paddr() -> usize {
    let satp = csr::satp();
    (satp & ((1usize << 44) - 1)) << RISCV_PG_SHIFT
}

/// Map a single 4 KiB user page at `vaddr` to `paddr` with given flags,
/// allocating intermediate PT levels from the boot pool as needed.
///
/// `flags` must include at least `PTE_V` and any of R/W/X to mark it as a
/// leaf entry. `PTE_U` is automatically added for user mappings.
///
/// Walking page tables during boot is tricky because the *only* mapping
/// guaranteed by the elfloader is the 1 GiB kernel-ELF window, while the
/// pages of the root PT itself can live anywhere. To keep things
/// well-defined we:
///
///   1. Allocate every new PT level from `BOOT_PT_POOL` (kernel-ELF window,
///      always accessible).
///   2. When following an existing entry, assume it points into the boot
///      pool too (true because *we* allocated everything below the root).
///
/// We must NOT chase entries installed by the elfloader itself, but the
/// elfloader only ever wrote the kernel-window L1 entries, never user-space
/// L2/L1 chains.
pub unsafe fn map_user_4k(root: *mut PageTable, vaddr: usize, paddr: usize, flags: u64) {
    let _guard = lock_vspace(root);
    unsafe { map_user_4k_locked(root, vaddr, paddr, flags) };
}

unsafe fn map_user_4k_locked(root: *mut PageTable, vaddr: usize, paddr: usize, mut flags: u64) {
    debug_assert!(vaddr & (PAGE_SIZE - 1) == 0, "vaddr not 4K-aligned");
    debug_assert!(paddr & (PAGE_SIZE - 1) == 0, "paddr not 4K-aligned");
    flags |= PTE_U | PTE_V | PTE_A | PTE_D;

    let mut pt = root;
    let mut l1pt_pa = 0u64;
    let mut l0pt_pa = 0u64;
    for level in (1..=2).rev() {
        let i = pt_index(vaddr, level);
        let entry = unsafe { (*pt).entries[i] };
        let next_pt: *mut PageTable = if !entry.is_valid() {
            let new_pt = alloc_pt_page();
            let new_pt_pa = kpptr_to_paddr(new_pt as usize) as u64;
            if level == 2 {
                l1pt_pa = new_pt_pa;
            } else {
                l0pt_pa = new_pt_pa;
            }
            unsafe {
                (*pt).entries[i] = Pte::next(new_pt_pa);
            }
            new_pt
        } else if entry.is_leaf() {
            panic!(
                "map_user_4k: collision with megapage at level {} for VA {:#x}",
                level, vaddr
            );
        } else {
            let pa = entry.next_pt_paddr();
            if level == 2 {
                l1pt_pa = pa;
            } else {
                l0pt_pa = pa;
            }
            paddr_to_kpptr(pa as usize) as *mut PageTable
        };
        pt = next_pt;
    }

    let i = pt_index(vaddr, 0);
    let _ = (l1pt_pa, l0pt_pa);
    unsafe {
        (*pt).entries[i] = Pte::leaf(paddr as u64, flags);
    }
    csr::sfence_vma_va(vaddr);
    crate::kernel::smp::remote_sfence_vma_all();
}

#[inline]
pub const fn frame_size_bytes(size_class: u64) -> usize {
    match size_class {
        1 => 1 << (RISCV_PG_SHIFT + PT_INDEX_BITS),
        2 => 1 << (RISCV_PG_SHIFT + PT_INDEX_BITS * 2),
        _ => PAGE_SIZE,
    }
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
        0 => Some(RISCV_PG_SHIFT),
        1 => Some(RISCV_PG_SHIFT + PT_INDEX_BITS),
        2 => Some(RISCV_PG_SHIFT + PT_INDEX_BITS * 2),
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
    let mut bits_left = RISCV_PG_SHIFT + PT_INDEX_BITS * 2;
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

/// Map a RISC-V frame cap at its natural Sv39 level:
///
/// * size class 0: 4 KiB leaf at level 0
/// * size class 1: 2 MiB leaf at level 1
/// * size class 2: 1 GiB leaf at level 2
pub unsafe fn map_user_frame(
    root: *mut PageTable,
    vaddr: usize,
    paddr: usize,
    size_class: u64,
    flags: u64,
) -> Result<(), UserMapError> {
    unsafe { map_user_frame_at(root, vaddr, paddr, size_class, flags, false) }
}

pub unsafe fn remap_user_frame(
    root: *mut PageTable,
    vaddr: usize,
    paddr: usize,
    size_class: u64,
    flags: u64,
) -> Result<(), UserMapError> {
    unsafe { map_user_frame_at(root, vaddr, paddr, size_class, flags, true) }
}

unsafe fn map_user_frame_at(
    root: *mut PageTable,
    vaddr: usize,
    paddr: usize,
    size_class: u64,
    flags: u64,
    replace_existing_leaf: bool,
) -> Result<(), UserMapError> {
    let prepared = unsafe {
        prepare_user_frame_map_at(root, vaddr, paddr, size_class, flags, replace_existing_leaf)?
    };
    unsafe { commit_user_frame_map(prepared) };
    Ok(())
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
    csr::sfence_vma_va(prepared.vaddr);
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
    if lookup.bits_left == RISCV_PG_SHIFT || entry.is_valid() {
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
    csr::sfence_vma_va(prepared.mapped_addr);
    crate::kernel::smp::remote_sfence_vma_all();
}

pub unsafe fn map_user_page_table(
    root: *mut PageTable,
    vaddr: usize,
    pt_kva: *mut PageTable,
) -> Result<usize, UserMapError> {
    let prepared = unsafe { prepare_user_page_table_map(root, vaddr, pt_kva)? };
    let mapped_addr = prepared.mapped_addr;
    unsafe { commit_user_page_table_map(prepared) };
    Ok(mapped_addr)
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
            csr::sfence_vma_all();
            crate::kernel::smp::remote_sfence_vma_all();
            return true;
        }
        pt = next_pt;
    }
    false
}

/// Remove the 4 KiB user mapping at `vaddr` if present and trim any
/// interior PT levels that become empty as a result.  Returns the
/// physical address the page used to map to, or `None` if no mapping
/// existed.
///
/// Recycling empty L1/L0 tables back onto the boot pool's freelist is
/// what keeps `BOOT_PT_POOL` from growing unbounded across the
/// rootserver's test sweep — every test process eventually has its
/// frames unmapped via `seL4_RISCV_Page_Unmap` (directly or via the
/// Revoke walk in `finalize_cap`), so this is also where we close the
/// loop on per-VSpace cleanup.
///
/// We follow the same "only chase entries we allocated" invariant as
/// `map_user_4k`: every interior PTE is expected to live in the boot
/// pool, so its physical address can be translated back to a kernel-ELF
/// VA via `paddr_to_kpptr`.
pub unsafe fn unmap_user_4k(root: *mut PageTable, vaddr: usize) -> Option<usize> {
    let _guard = lock_vspace(root);
    unsafe { unmap_user_4k_locked(root, vaddr) }
}

unsafe fn unmap_user_4k_locked(root: *mut PageTable, vaddr: usize) -> Option<usize> {
    debug_assert!(vaddr & (PAGE_SIZE - 1) == 0, "vaddr not 4K-aligned");

    // Remember each interior table we walked so we can prune the
    // empties on the way back up.  `pts[level]` is the table that
    // *contains* `pt_index(vaddr, level)`; `pts[2]` is the root and is
    // never freed.
    let mut pts: [*mut PageTable; 3] = [core::ptr::null_mut(); 3];
    pts[2] = root;
    let mut pt = root;
    for level in (1..=2).rev() {
        let i = pt_index(vaddr, level);
        let entry = unsafe { (*pt).entries[i] };
        if !entry.is_valid() || entry.is_leaf() {
            return None;
        }
        pt = paddr_to_kpptr(entry.next_pt_paddr() as usize) as *mut PageTable;
        pts[level - 1] = pt;
    }

    let i = pt_index(vaddr, 0);
    let entry = unsafe { (*pt).entries[i] };
    if !entry.is_valid() || !entry.is_leaf() {
        return None;
    }
    let pa = entry.leaf_pa() as usize;
    unsafe {
        (*pt).entries[i] = Pte::NULL;
    }
    csr::sfence_vma_va(vaddr);
    crate::kernel::smp::remote_sfence_vma_all();

    // Walk up, freeing each interior table that no longer references
    // anything.  We *never* trim the root (`level == 2`): it carries
    // the kernel-ELF + PSpace mappings every process depends on.
    for level in 0..=1 {
        let child = pts[level];
        if unsafe { !pt_is_empty(child) } {
            break;
        }
        let parent = pts[level + 1];
        let parent_i = pt_index(vaddr, level + 1);
        unsafe {
            (*parent).entries[parent_i] = Pte::NULL;
            free_pt_page(child);
        }
    }
    Some(pa)
}

/// Remove a user frame mapping at the natural Sv39 level for the cap's
/// size class. Unlike the old boot-pool helper, this does not try to
/// reclaim interior page-table pages: user PageTable caps manage those
/// explicitly.
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
    csr::sfence_vma_va(vaddr);
    crate::kernel::smp::remote_sfence_vma_all();
    Some(pa)
}

/// Clear the user half of a root page table recursively.
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
    csr::sfence_vma_all();
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

/// True if every entry in `pt` is invalid (i.e. the table contributes
/// no mappings).  Cheap because `Pte` is `Copy`.
#[inline]
unsafe fn pt_is_empty(pt: *mut PageTable) -> bool {
    for i in 0..512 {
        if unsafe { (*pt).entries[i] }.is_valid() {
            return false;
        }
    }
    true
}

/// Install a fresh `satp` value, then flush the TLB.
pub unsafe fn switch_satp(satp_val: u64) {
    csr::sfence_vma_all();
    csr::set_satp(satp_val as usize);
    csr::sfence_vma_all();
    csr::fence_i();
}

fn switch_to_kernel_root() {
    let Some(kernel_satp) = crate::kernel::smp::kernel_satp() else {
        return;
    };
    if csr::satp() as u64 != kernel_satp {
        unsafe { switch_satp(kernel_satp) };
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
    if csr::satp() as u64 != new_satp {
        unsafe { switch_satp(new_satp) };
    }
    true
}

/// Mirror RISC-V seL4 `setVMRoot(ksCurThread)`: after ASID deletion,
/// re-evaluate the current TCB's VSpace and fall back to the kernel root
/// if its page-table cap no longer resolves through the ASID table.
pub fn set_current_vspace_root() {
    let current = crate::object::tcb::current();
    if !try_switch_to_tcb_root(current) {
        switch_to_kernel_root();
    }
}

/// `seL4` user permissions ⇒ Sv39 PTE flag bits (for U-mode, 4K page).
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
    unsafe {
        (*pt).entries[kernel_l2_index] = Pte::leaf(kernel_pa, kernel_flags);
    }
    let pspace_base_l2 = pt_index(PPTR_BASE, 2);
    for i in 0..8 {
        let pa = (i as u64) * (1u64 << 30);
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
/// User mappings (≤ L2[255]) are filled in later via `map_user_4k`.
pub fn make_boot_root_pt() -> *mut PageTable {
    let root = alloc_pt_page();
    let kernel_flags = PTE_V | PTE_R | PTE_W | PTE_X | PTE_G | PTE_A | PTE_D;
    let pspace_flags = PTE_V | PTE_R | PTE_W | PTE_G | PTE_A | PTE_D;
    let _guard = lock_vspace(root);

    let kernel_l2_index = pt_index(KERNEL_ELF_BASE, 2);
    let kernel_pa = 0x8000_0000u64;
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
        unsafe {
            (*root).entries[pspace_base_l2 + i] = Pte::leaf(pa, pspace_flags);
        }
    }
    root
}

/// Compose a Sv39 `satp` value for the given root PT (kernel-ELF VA) and
/// ASID, by translating its VA to its physical address.
pub fn satp_for(root: *mut PageTable, asid: u64) -> u64 {
    let pa = kpptr_to_paddr(root as usize) as u64;
    make_satp(asid, pa)
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
    use crate::abi::constants::{KERNEL_ELF_BASE, PPTR_BASE, PPTR_TOP};
    let kva = root_kva as usize;
    let pa = if kva >= PPTR_BASE && kva < PPTR_TOP {
        pptr_to_paddr(kva)
    } else if kva >= KERNEL_ELF_BASE {
        kpptr_to_paddr(kva)
    } else {
        return 0;
    };
    make_satp(asid, pa as u64)
}
