//! Tiny bump allocator for boot-time kernel data structures.
//!
//! Everything we allocate here lives inside a fixed 3 MiB pool in the
//! kernel ELF's BSS. The pool is 4 KiB-aligned so callers can hand pages
//! back to user-space via the active architecture's paging objects.

use crate::arch::current::machine::paging::PAGE_SIZE;
use crate::kernel::smp::BklCell;

const BOOT_POOL_PAGES: usize = 768; // 3 MiB

#[repr(C, align(4096))]
struct BootPool {
    bytes: [u8; BOOT_POOL_PAGES * PAGE_SIZE],
    next: usize,
}

impl BootPool {
    const fn new() -> Self {
        Self {
            bytes: [0u8; BOOT_POOL_PAGES * PAGE_SIZE],
            next: 0,
        }
    }

    fn alloc_pages(&mut self, n: usize) -> usize {
        let bytes = n.checked_mul(PAGE_SIZE).expect("boot pool size overflow");
        let end = self.next.checked_add(bytes).expect("boot pool exhausted");
        assert!(end <= BOOT_POOL_PAGES * PAGE_SIZE, "boot pool exhausted");
        let base = unsafe { self.bytes.as_mut_ptr().add(self.next) };
        self.next = end;
        // Zero the range we just claimed.
        unsafe { core::ptr::write_bytes(base, 0, bytes) };
        base as usize
    }
}

static BOOT_POOL: BklCell<BootPool> = BklCell::new(BootPool::new());

/// Allocate `n` contiguous 4 KiB pages, zeroed. Returns the kernel-ELF VA
/// of the first byte.
pub fn alloc_pages(n: usize) -> usize {
    BOOT_POOL.with_mut(|pool| pool.alloc_pages(n))
}

#[inline]
pub fn alloc_page() -> usize {
    alloc_pages(1)
}
