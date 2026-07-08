//! Sv39 page table primitives.
//!
//! Sv39 has 3 levels, 9-bit indices, 4 KiB pages, and 2 MiB / 1 GiB megapages.
//! A page table entry packs the PPN of the next table or leaf frame plus
//! permission bits (V, R, W, X, U, G, A, D).

#![allow(dead_code)]

use crate::abi::constants::{PT_INDEX_BITS, RISCV_PG_SHIFT, SEL4_PAGE_TABLE_ENTRIES};

pub const PAGE_SHIFT: usize = RISCV_PG_SHIFT;
pub const PAGE_SIZE: usize = 1 << RISCV_PG_SHIFT; // 4096
pub const LEAF_LEVEL: usize = 0;
pub const ROOT_LEVEL: usize = 2;
pub const ROOT_CHILD_COVERAGE_BITS: usize = RISCV_PG_SHIFT + PT_INDEX_BITS * 2;
pub const LEAF_PARENT_COVERAGE_BITS: usize = RISCV_PG_SHIFT + PT_INDEX_BITS;

// Levels: 0 = leaf (4K), 1 = 2MiB, 2 = 1GiB.
// (Confusingly, the seL4 C code numbers levels the other way; we use
// "level 0 = leaf" throughout this file for clarity.)

// ---- PTE bit flags --------------------------------------------------------
pub const PTE_V: u64 = 1 << 0; // Valid
pub const PTE_R: u64 = 1 << 1; // Read
pub const PTE_W: u64 = 1 << 2; // Write
pub const PTE_X: u64 = 1 << 3; // Execute
pub const PTE_U: u64 = 1 << 4; // User accessible
pub const PTE_G: u64 = 1 << 5; // Global
pub const PTE_A: u64 = 1 << 6; // Accessed
pub const PTE_D: u64 = 1 << 7; // Dirty

/// Leaf permissions for kernel-window pages (R/W/X, supervisor, global).
pub const PTE_KERNEL_RWX: u64 = PTE_V | PTE_R | PTE_W | PTE_X | PTE_G | PTE_A | PTE_D;
/// Leaf permissions for user data pages (R/W, user, accessed/dirty).
pub const PTE_USER_RW: u64 = PTE_V | PTE_R | PTE_W | PTE_U | PTE_A | PTE_D;
/// Leaf permissions for user code pages (R/X, user).
pub const PTE_USER_RX: u64 = PTE_V | PTE_R | PTE_X | PTE_U | PTE_A | PTE_D;
/// Leaf permissions for user RWX pages (used during boot-mapping when we
/// don't separate text/data — kept tight enough not to leak).
pub const PTE_USER_RWX: u64 = PTE_V | PTE_R | PTE_W | PTE_X | PTE_U | PTE_A | PTE_D;

/// A single 64-bit page table entry.
#[repr(transparent)]
#[derive(Copy, Clone)]
pub struct Pte(pub u64);

impl Pte {
    pub const NULL: Pte = Pte(0);

    /// Wrap a raw 64-bit word as a PTE. Useful for embedding freelist
    /// pointers inside reclaimed PT pages.
    #[inline]
    pub const fn from_raw(raw: u64) -> Pte {
        Pte(raw)
    }

    /// Inverse of `from_raw`.
    #[inline]
    pub const fn raw(self) -> u64 {
        self.0
    }

    /// Build a non-leaf entry pointing at the next-level PT (which must be
    /// 4 KiB aligned).
    #[inline]
    pub const fn next(pt_paddr: u64) -> Pte {
        // V = 1, R = W = X = 0 marks "table pointer".
        Pte(((pt_paddr >> RISCV_PG_SHIFT) << 10) | PTE_V)
    }

    /// Build a leaf entry mapping `paddr` with the given permission flags.
    #[inline]
    pub const fn leaf(paddr: u64, flags: u64) -> Pte {
        Pte(((paddr >> RISCV_PG_SHIFT) << 10) | flags)
    }

    #[inline]
    pub const fn is_valid(self) -> bool {
        (self.0 & PTE_V) != 0
    }

    #[inline]
    pub const fn is_leaf(self) -> bool {
        // A leaf has at least one of R/W/X set in addition to V.
        (self.0 & PTE_V) != 0 && (self.0 & (PTE_R | PTE_W | PTE_X)) != 0
    }

    #[inline]
    pub const fn ppn(self) -> u64 {
        (self.0 >> 10) & ((1u64 << 44) - 1)
    }

    #[inline]
    pub const fn next_pt_paddr(self) -> u64 {
        self.ppn() << RISCV_PG_SHIFT
    }

    /// Physical address of the leaf page this PTE maps to. Only
    /// meaningful when `is_leaf()` holds.
    #[inline]
    pub const fn leaf_pa(self) -> u64 {
        self.ppn() << RISCV_PG_SHIFT
    }
}

/// A single Sv39 page table — 512 entries, 4 KiB total.
#[repr(C, align(4096))]
pub struct PageTable {
    pub entries: [Pte; SEL4_PAGE_TABLE_ENTRIES],
}

impl PageTable {
    pub const fn zeroed() -> Self {
        PageTable {
            entries: [Pte::NULL; SEL4_PAGE_TABLE_ENTRIES],
        }
    }
}

const _: () = {
    assert!(core::mem::size_of::<PageTable>() == PAGE_SIZE);
    assert!(core::mem::align_of::<PageTable>() == PAGE_SIZE);
};

/// Index of `vaddr` at PT level `level` (0 = leaf, 2 = root).
#[inline]
pub const fn pt_index(vaddr: usize, level: usize) -> usize {
    (vaddr >> (RISCV_PG_SHIFT + PT_INDEX_BITS * level)) & ((1 << PT_INDEX_BITS) - 1)
}

/// Construct an `satp` value for Sv39 with the given ASID and PT physical
/// address. Sv39 mode = 8.
#[inline]
pub const fn make_satp(asid: u64, root_pt_paddr: u64) -> u64 {
    (8u64 << 60)
        | ((asid & 0xFFFF) << 44)
        | ((root_pt_paddr >> RISCV_PG_SHIFT) & ((1u64 << 44) - 1))
}
