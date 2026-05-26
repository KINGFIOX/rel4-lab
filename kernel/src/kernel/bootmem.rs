//! Tiny bump allocator for boot-time kernel data structures.
//!
//! Everything we allocate here lives inside a fixed 1 MiB pool in the
//! kernel ELF's BSS. The pool is 4 KiB-aligned so callers can hand pages
//! back to user-space via Sv39 mappings.

use core::sync::atomic::{AtomicUsize, Ordering};

use crate::arch::riscv64::sv39::PAGE_SIZE;

const BOOT_POOL_PAGES: usize = 256; // 1 MiB

#[repr(C, align(4096))]
struct BootPool {
    bytes: [u8; BOOT_POOL_PAGES * PAGE_SIZE],
}

static mut BOOT_POOL: BootPool = BootPool {
    bytes: [0u8; BOOT_POOL_PAGES * PAGE_SIZE],
};

static BOOT_POOL_NEXT: AtomicUsize = AtomicUsize::new(0);

/// Allocate `n` contiguous 4 KiB pages, zeroed. Returns the kernel-ELF VA
/// of the first byte.
pub fn alloc_pages(n: usize) -> usize {
    let offset = BOOT_POOL_NEXT.fetch_add(n * PAGE_SIZE, Ordering::SeqCst);
    assert!(
        offset + n * PAGE_SIZE <= BOOT_POOL_PAGES * PAGE_SIZE,
        "boot pool exhausted",
    );
    let base = unsafe { (&raw mut BOOT_POOL.bytes[0]).add(offset) };
    // Zero the range we just claimed.
    unsafe { core::ptr::write_bytes(base, 0, n * PAGE_SIZE) };
    base as usize
}

#[inline]
pub fn alloc_page() -> usize {
    alloc_pages(1)
}
