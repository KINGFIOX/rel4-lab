//! Kernel + user VSpace helpers.
//!
//! For M2 we keep things very simple: the elfloader hands us a Sv39 root PT
//! already containing the 1 GiB kernel-window mapping. We simply walk that
//! page table to install user-image mappings, allocating fresh 4 KiB PT
//! pages out of a static kernel boot pool when needed.

use core::sync::atomic::{AtomicUsize, Ordering};

use crate::abi::constants::{
    KERNEL_ELF_BASE, PADDR_BASE, PHYS_BASE_RAW, PPTR_BASE, PPTR_TOP, PT_INDEX_BITS,
    RISCV_PG_SHIFT,
};
use crate::arch::riscv64::csr;
use crate::arch::riscv64::sv39::{
    PAGE_SIZE, PTE_A, PTE_D, PTE_G, PTE_R, PTE_U, PTE_V, PTE_W, PTE_X, PageTable, Pte, make_satp,
    pt_index,
};

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

// The boot pool keeps L1/L0 page-table pages for *every* user VSpace
// the rootserver creates. Each Page_Map call can pull up to two fresh
// pages out of this pool; without recycling, a 100-test sweep blows
// past anything we'd ship in `.bss`. We keep the pool modest (128
// pages ≈ 512 KiB) and reclaim empty interior tables back onto the
// `BOOT_PT_FREELIST` stack as soon as `unmap_user_4k` empties them.
//
// `seL4` would normally manage these via user-supplied `PageTable`
// objects (Retype-from-Untyped); modelling that properly is on the
// M4 todo list.
const BOOT_PT_POOL_PAGES: usize = 128;

#[repr(C, align(4096))]
struct BootPtPool {
    pages: [PageTable; BOOT_PT_POOL_PAGES],
}

static mut BOOT_PT_POOL: BootPtPool = BootPtPool {
    pages: [const { PageTable::zeroed() }; BOOT_PT_POOL_PAGES],
};

/// Bump pointer for pages we haven't handed out yet. We only consult
/// this when the freelist is empty.
static BOOT_PT_POOL_NEXT: AtomicUsize = AtomicUsize::new(0);

/// Freelist head — a sentinel index (`!0` ≡ empty) into the bump pool.
/// Recycled PT pages embed the next-free index in the first PTE's
/// raw bits (we re-zero the slot before handing it back out).
static BOOT_PT_FREELIST_HEAD: AtomicUsize = AtomicUsize::new(usize::MAX);

#[inline]
fn pool_base() -> *mut PageTable {
    // SAFETY: we only ever construct a raw pointer from the static
    // pool; deref happens through the freelist / bump-pointer paths,
    // which run single-threaded on the rootserver hart.
    unsafe { &raw mut BOOT_PT_POOL.pages as *mut PageTable }
}

/// True if `p` was allocated from `BOOT_PT_POOL`.
#[inline]
fn is_boot_pool_pt(p: *mut PageTable) -> bool {
    let base = pool_base() as usize;
    let end = base + BOOT_PT_POOL_PAGES * core::mem::size_of::<PageTable>();
    let v = p as usize;
    v >= base && v < end && (v - base) % core::mem::size_of::<PageTable>() == 0
}

/// Allocate a fresh zeroed page-table page from the boot pool. Returns its
/// kernel-window virtual address. Prefers the freelist over the bump
/// pointer so long-running suites don't starve the pool.
pub fn alloc_pt_page() -> *mut PageTable {
    let head = BOOT_PT_FREELIST_HEAD.load(Ordering::SeqCst);
    if head != usize::MAX {
        unsafe {
            let p = pool_base().add(head);
            // Next-free index is stashed in entries[0].
            let next = (*p).entries[0].raw() as usize;
            BOOT_PT_FREELIST_HEAD.store(next, Ordering::SeqCst);
            (*p).entries = [Pte::NULL; 512];
            return p;
        }
    }
    let idx = BOOT_PT_POOL_NEXT.fetch_add(1, Ordering::SeqCst);
    assert!(idx < BOOT_PT_POOL_PAGES, "boot PT pool exhausted");
    unsafe {
        let p = pool_base().add(idx);
        (*p).entries = [Pte::NULL; 512];
        p
    }
}

/// Push a PT page back onto the boot-pool freelist. Silently ignores
/// pages outside the pool (e.g. caller-owned page-table objects we
/// never allocated).
pub unsafe fn free_pt_page(p: *mut PageTable) {
    if !is_boot_pool_pt(p) {
        return;
    }
    let idx = ((p as usize) - (pool_base() as usize)) / core::mem::size_of::<PageTable>();
    unsafe {
        let head = BOOT_PT_FREELIST_HEAD.load(Ordering::SeqCst);
        (*p).entries[0] = Pte::from_raw(head as u64);
        // Zero the rest so a stale entry can't accidentally look valid.
        for i in 1..512 {
            (*p).entries[i] = Pte::NULL;
        }
        BOOT_PT_FREELIST_HEAD.store(idx, Ordering::SeqCst);
    }
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
pub unsafe fn map_user_4k(root: *mut PageTable, vaddr: usize, paddr: usize, mut flags: u64) {
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
}

#[inline]
pub const fn frame_size_bytes(size_class: u64) -> usize {
    match size_class {
        1 => 1 << (RISCV_PG_SHIFT + PT_INDEX_BITS),
        2 => 1 << (RISCV_PG_SHIFT + PT_INDEX_BITS * 2),
        _ => PAGE_SIZE,
    }
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
) {
    match size_class {
        0 => unsafe { map_user_4k(root, vaddr, paddr, flags) },
        1 | 2 => unsafe { map_user_leaf(root, vaddr, paddr, size_class as usize, flags) },
        _ => unsafe { map_user_4k(root, vaddr, paddr, flags) },
    }
}

unsafe fn map_user_leaf(
    root: *mut PageTable,
    vaddr: usize,
    paddr: usize,
    level: usize,
    mut flags: u64,
) {
    let size = 1usize << (RISCV_PG_SHIFT + PT_INDEX_BITS * level);
    debug_assert!(vaddr & (size - 1) == 0, "vaddr not frame-aligned");
    debug_assert!(paddr & (size - 1) == 0, "paddr not frame-aligned");
    flags |= PTE_U | PTE_V | PTE_A | PTE_D;

    let mut pt = root;
    for walk_level in ((level + 1)..=2).rev() {
        let i = pt_index(vaddr, walk_level);
        let entry = unsafe { (*pt).entries[i] };
        let next_pt = if !entry.is_valid() {
            let new_pt = alloc_pt_page();
            let new_pt_pa = kpptr_to_paddr(new_pt as usize) as u64;
            unsafe {
                (*pt).entries[i] = Pte::next(new_pt_pa);
            }
            new_pt
        } else if entry.is_leaf() {
            panic!(
                "map_user_frame: leaf collision at level {} for VA {:#x}",
                walk_level, vaddr
            );
        } else {
            paddr_to_kpptr(entry.next_pt_paddr() as usize) as *mut PageTable
        };
        pt = next_pt;
    }

    let i = pt_index(vaddr, level);
    let entry = unsafe { (*pt).entries[i] };
    if entry.is_valid() && !entry.is_leaf() {
        panic!(
            "map_user_frame: page-table collision at level {} for VA {:#x}",
            level, vaddr
        );
    }
    unsafe {
        (*pt).entries[i] = Pte::leaf(paddr as u64, flags);
    }
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
/// size class, pruning now-empty boot-pool page-table pages on the way
/// back up.
pub unsafe fn unmap_user_frame(
    root: *mut PageTable,
    vaddr: usize,
    size_class: u64,
) -> Option<usize> {
    match size_class {
        0 => unsafe { unmap_user_4k(root, vaddr) },
        1 | 2 => unsafe { unmap_user_leaf(root, vaddr, size_class as usize) },
        _ => unsafe { unmap_user_4k(root, vaddr) },
    }
}

unsafe fn unmap_user_leaf(
    root: *mut PageTable,
    vaddr: usize,
    level: usize,
) -> Option<usize> {
    let size = 1usize << (RISCV_PG_SHIFT + PT_INDEX_BITS * level);
    debug_assert!(vaddr & (size - 1) == 0, "vaddr not frame-aligned");

    let mut pts: [*mut PageTable; 3] = [core::ptr::null_mut(); 3];
    pts[2] = root;
    let mut pt = root;
    for walk_level in ((level + 1)..=2).rev() {
        let i = pt_index(vaddr, walk_level);
        let entry = unsafe { (*pt).entries[i] };
        if !entry.is_valid() || entry.is_leaf() {
            return None;
        }
        pt = paddr_to_kpptr(entry.next_pt_paddr() as usize) as *mut PageTable;
        pts[walk_level - 1] = pt;
    }

    let i = pt_index(vaddr, level);
    let entry = unsafe { (*pt).entries[i] };
    if !entry.is_valid() || !entry.is_leaf() {
        return None;
    }
    let pa = entry.leaf_pa() as usize;
    unsafe {
        (*pt).entries[i] = Pte::NULL;
    }
    csr::sfence_vma_va(vaddr);

    let mut child_level = level;
    while child_level < 2 {
        let child = pts[child_level];
        if child.is_null() || unsafe { !pt_is_empty(child) } {
            break;
        }
        let parent = pts[child_level + 1];
        let parent_i = pt_index(vaddr, child_level + 1);
        unsafe {
            (*parent).entries[parent_i] = Pte::NULL;
            free_pt_page(child);
        }
        child_level += 1;
    }
    Some(pa)
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
