//! Kernel + user VSpace helpers.
//!
//! For M2 we keep things very simple: the elfloader hands us a Sv39 root PT
//! already containing the 1 GiB kernel-window mapping. We simply walk that
//! page table to install user-image mappings, allocating fresh 4 KiB PT
//! pages out of a static kernel boot pool when needed.

use core::sync::atomic::{AtomicUsize, Ordering};

use crate::abi::constants::{
    KERNEL_ELF_BASE, PADDR_BASE, PHYS_BASE_RAW, PPTR_BASE, PPTR_TOP, RISCV_PG_SHIFT,
};
use crate::arch::riscv64::csr;
use crate::arch::riscv64::sv39::{
    PAGE_SIZE, PageTable, Pte, PTE_A, PTE_D, PTE_G, PTE_R, PTE_U, PTE_V, PTE_W, PTE_X,
    make_satp, pt_index,
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

const BOOT_PT_POOL_PAGES: usize = 64;

#[repr(C, align(4096))]
struct BootPtPool {
    pages: [PageTable; BOOT_PT_POOL_PAGES],
}

static mut BOOT_PT_POOL: BootPtPool = BootPtPool {
    pages: [const { PageTable::zeroed() }; BOOT_PT_POOL_PAGES],
};

static BOOT_PT_POOL_NEXT: AtomicUsize = AtomicUsize::new(0);

/// Allocate a fresh zeroed page-table page from the boot pool. Returns its
/// kernel-window virtual address.
pub fn alloc_pt_page() -> *mut PageTable {
    let idx = BOOT_PT_POOL_NEXT.fetch_add(1, Ordering::SeqCst);
    assert!(idx < BOOT_PT_POOL_PAGES, "boot PT pool exhausted");
    unsafe {
        let p = &raw mut BOOT_PT_POOL.pages[idx];
        // Zero it (was zeroed by .bss clear, but be defensive in case we
        // ever recycle).
        (*p).entries = [Pte::NULL; 512];
        p
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
    for level in (1..=2).rev() {
        let i = pt_index(vaddr, level);
        let entry = unsafe { (*pt).entries[i] };
        let next_pt: *mut PageTable = if !entry.is_valid() {
            let new_pt = alloc_pt_page();
            let new_pt_pa = kpptr_to_paddr(new_pt as usize) as u64;
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
            paddr_to_kpptr(entry.next_pt_paddr() as usize) as *mut PageTable
        };
        pt = next_pt;
    }

    let i = pt_index(vaddr, 0);
    unsafe {
        (*pt).entries[i] = Pte::leaf(paddr as u64, flags);
    }
}

/// Remove the 4 KiB user mapping at `vaddr` if present, leaving any
/// intermediate page-table levels alone. Returns the physical address
/// the page used to map to, or `None` if no mapping existed.
///
/// We follow the same "only chase entries we allocated" invariant as
/// `map_user_4k`: every interior PTE is expected to live in the boot
/// pool, so its physical address can be translated back to a kernel-ELF
/// VA via `paddr_to_kpptr`.
pub unsafe fn unmap_user_4k(root: *mut PageTable, vaddr: usize) -> Option<usize> {
    debug_assert!(vaddr & (PAGE_SIZE - 1) == 0, "vaddr not 4K-aligned");

    let mut pt = root;
    for level in (1..=2).rev() {
        let i = pt_index(vaddr, level);
        let entry = unsafe { (*pt).entries[i] };
        if !entry.is_valid() || entry.is_leaf() {
            return None;
        }
        pt = paddr_to_kpptr(entry.next_pt_paddr() as usize) as *mut PageTable;
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
    Some(pa)
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
    if read { f |= PTE_R; }
    if write { f |= PTE_W; }
    if exec { f |= PTE_X; }
    f
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
