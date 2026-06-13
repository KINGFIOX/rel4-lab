//! LoongArch64 page-table type skeleton.
//!
//! LoongArch64 uses a different TLB/page-table model from RISC-V Sv39. This
//! module intentionally only provides the public shape required by the shared
//! kernel object code while the real LoongArch VSpace backend is being ported.

use crate::abi::constants::{PT_INDEX_BITS, SEL4_PAGE_BITS, SEL4_PAGE_TABLE_ENTRIES};

pub const PAGE_SHIFT: usize = SEL4_PAGE_BITS;
pub const PAGE_SIZE: usize = 1 << PAGE_SHIFT;
pub const LEAF_LEVEL: usize = 0;
pub const ROOT_LEVEL: usize = 2;
pub const ROOT_CHILD_COVERAGE_BITS: usize = PAGE_SHIFT + PT_INDEX_BITS * 2;
pub const LEAF_PARENT_COVERAGE_BITS: usize = PAGE_SHIFT + PT_INDEX_BITS;

pub const PTE_V: u64 = 1 << 0;
pub const PTE_R: u64 = 1 << 1;
pub const PTE_W: u64 = 1 << 2;
pub const PTE_X: u64 = 1 << 3;
pub const PTE_U: u64 = 1 << 4;
pub const PTE_G: u64 = 1 << 5;
pub const PTE_A: u64 = 1 << 6;
pub const PTE_D: u64 = 1 << 7;

pub const PTE_KERNEL_RWX: u64 = PTE_V | PTE_R | PTE_W | PTE_X | PTE_G | PTE_A | PTE_D;
pub const PTE_USER_RW: u64 = PTE_V | PTE_R | PTE_W | PTE_U | PTE_A | PTE_D;
pub const PTE_USER_RX: u64 = PTE_V | PTE_R | PTE_X | PTE_U | PTE_A | PTE_D;
pub const PTE_USER_RWX: u64 = PTE_V | PTE_R | PTE_W | PTE_X | PTE_U | PTE_A | PTE_D;

#[repr(transparent)]
#[derive(Copy, Clone)]
pub struct Pte(pub u64);

impl Pte {
    pub const NULL: Pte = Pte(0);

    #[inline]
    pub const fn from_raw(raw: u64) -> Pte {
        Pte(raw)
    }

    #[inline]
    pub const fn raw(self) -> u64 {
        self.0
    }

    #[inline]
    pub const fn next(pt_paddr: u64) -> Pte {
        Pte(((pt_paddr >> PAGE_SHIFT) << 10) | PTE_V)
    }

    #[inline]
    pub const fn leaf(paddr: u64, flags: u64) -> Pte {
        Pte(((paddr >> PAGE_SHIFT) << 10) | flags)
    }

    #[inline]
    pub const fn is_valid(self) -> bool {
        (self.0 & PTE_V) != 0
    }

    #[inline]
    pub const fn is_leaf(self) -> bool {
        (self.0 & PTE_V) != 0 && (self.0 & (PTE_R | PTE_W | PTE_X)) != 0
    }

    #[inline]
    pub const fn ppn(self) -> u64 {
        (self.0 >> 10) & ((1u64 << 44) - 1)
    }

    #[inline]
    pub const fn next_pt_paddr(self) -> u64 {
        self.ppn() << PAGE_SHIFT
    }

    #[inline]
    pub const fn leaf_pa(self) -> u64 {
        self.ppn() << PAGE_SHIFT
    }
}

#[repr(C, align(4096))]
pub struct PageTable {
    pub entries: [Pte; SEL4_PAGE_TABLE_ENTRIES],
}

impl PageTable {
    pub const fn zeroed() -> Self {
        Self {
            entries: [Pte::NULL; SEL4_PAGE_TABLE_ENTRIES],
        }
    }
}

const _: () = {
    assert!(core::mem::size_of::<PageTable>() == PAGE_SIZE);
    assert!(core::mem::align_of::<PageTable>() == PAGE_SIZE);
};

#[inline]
pub const fn pt_index(vaddr: usize, level: usize) -> usize {
    (vaddr >> (PAGE_SHIFT + PT_INDEX_BITS * level)) & ((1 << PT_INDEX_BITS) - 1)
}
