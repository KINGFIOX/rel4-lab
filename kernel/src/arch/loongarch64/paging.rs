//! LoongArch64 page-table entry encoding.
//!
//! The software page-table shape still mirrors the repository's three-level
//! seL4 object model, but the entry bits use LoongArch's TLB EntryLo format
//! plus the software `Present`/`Write` bits used by LoongArch leaf PTEs.

use crate::abi::constants::{PT_INDEX_BITS, SEL4_PAGE_BITS, SEL4_PAGE_TABLE_ENTRIES};

pub const PAGE_SHIFT: usize = SEL4_PAGE_BITS;
pub const PAGE_SIZE: usize = 1 << PAGE_SHIFT;
pub const LEAF_LEVEL: usize = 0;
pub const ROOT_LEVEL: usize = 2;
pub const ROOT_CHILD_COVERAGE_BITS: usize = PAGE_SHIFT + PT_INDEX_BITS * 2;
pub const LEAF_PARENT_COVERAGE_BITS: usize = PAGE_SHIFT + PT_INDEX_BITS;

pub const PTE_V: u64 = 1 << 0;
pub const PTE_D: u64 = 1 << 1;
pub const PTE_PLV_SHIFT: u64 = 2;
pub const PTE_PLV_MASK: u64 = 0b11 << PTE_PLV_SHIFT;
pub const PTE_PLV_KERNEL: u64 = 0b00 << PTE_PLV_SHIFT;
pub const PTE_PLV_USER: u64 = 0b11 << PTE_PLV_SHIFT;
pub const PTE_MAT_SHIFT: u64 = 4;
pub const PTE_MAT_SUC: u64 = 0b00 << PTE_MAT_SHIFT;
pub const PTE_MAT_CC: u64 = 0b01 << PTE_MAT_SHIFT;
pub const PTE_MAT_WUC: u64 = 0b10 << PTE_MAT_SHIFT;
pub const PTE_G: u64 = 1 << 6;
pub const PTE_HUGE: u64 = 1 << 6;
pub const PTE_PRESENT: u64 = 1 << 7;
pub const PTE_W: u64 = 1 << 8;
pub const PTE_MODIFIED: u64 = 1 << 9;
pub const PTE_SPECIAL: u64 = 1 << 11;
pub const PTE_PFN_SHIFT: u64 = 12;
pub const PTE_PFN_MASK: u64 = (1 << 36) - 1;
pub const PTE_NR: u64 = 1 << 61;
pub const PTE_NX: u64 = 1 << 62;
pub const PTE_RPLV: u64 = 1 << 63;

pub const PTE_KERNEL_RWX: u64 =
    PTE_PRESENT | PTE_V | PTE_D | PTE_W | PTE_G | PTE_PLV_KERNEL | PTE_MAT_CC;
pub const PTE_USER_RW: u64 =
    PTE_PRESENT | PTE_V | PTE_D | PTE_W | PTE_PLV_USER | PTE_MAT_CC | PTE_NX | PTE_RPLV;
pub const PTE_USER_RX: u64 = PTE_PRESENT | PTE_V | PTE_PLV_USER | PTE_MAT_CC | PTE_RPLV;
pub const PTE_USER_RWX: u64 =
    PTE_PRESENT | PTE_V | PTE_D | PTE_W | PTE_PLV_USER | PTE_MAT_CC | PTE_RPLV;

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
        Pte(pt_paddr & !((PAGE_SIZE as u64) - 1))
    }

    #[inline]
    pub const fn leaf(paddr: u64, flags: u64) -> Pte {
        Pte((paddr & !((PAGE_SIZE as u64) - 1)) | flags)
    }

    #[inline]
    pub const fn is_valid(self) -> bool {
        self.0 != 0
    }

    #[inline]
    pub const fn is_leaf(self) -> bool {
        (self.0 & PTE_PRESENT) != 0 && (self.0 & PTE_V) != 0
    }

    #[inline]
    pub const fn ppn(self) -> u64 {
        (self.0 >> PTE_PFN_SHIFT) & PTE_PFN_MASK
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
