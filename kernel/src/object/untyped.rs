//! Untyped memory tracking.
//!
//! Each contiguous chunk of free RAM that we report to userspace becomes
//! one or more `Untyped` caps. Their `capBlockSize` (log2 bytes) must be
//! in `[seL4_MinUntypedBits, seL4_MaxUntypedBits]` (4..=38 on RV64), and
//! `capPtr` is the kernel-window VA of the start of the region. The
//! region must be aligned to its size.
//!
//! For M3 we only implement the enumeration helpers — the actual
//! `Untyped_Retype` syscall path is a separate file (TBD).

#![allow(dead_code)]

use crate::abi::constants::{SEL4_MAX_UNTYPED_BITS, SEL4_MIN_UNTYPED_BITS};
use crate::object::cap::Cap;

/// A chunk of free physical memory that we still need to chop into
/// untyped caps for the rootserver.
#[derive(Copy, Clone, Debug)]
pub struct FreeRange {
    pub start_kva: u64, // kernel-window VA
    pub size: u64,
}

/// Iterator that splits a free range into power-of-two, naturally aligned
/// chunks ≤ 2^38 bytes — exactly the shape required by `cap_untyped_cap`.
pub struct UntypedChunks {
    cursor: u64,
    end: u64,
}

impl UntypedChunks {
    pub fn new(range: FreeRange) -> Self {
        Self {
            cursor: range.start_kva,
            end: range.start_kva + range.size,
        }
    }
}

impl Iterator for UntypedChunks {
    // `(base_kva, bits)`.
    type Item = (u64, u8);

    fn next(&mut self) -> Option<Self::Item> {
        if self.cursor >= self.end {
            return None;
        }

        let remaining = self.end - self.cursor;
        // Largest power of two ≤ remaining.
        let max_by_size = 63 - remaining.leading_zeros() as u8;
        // Largest power of two ≤ alignment of cursor.
        let max_by_align = if self.cursor == 0 {
            SEL4_MAX_UNTYPED_BITS as u8
        } else {
            self.cursor.trailing_zeros() as u8
        };
        let mut bits = core::cmp::min(max_by_size, max_by_align);
        bits = core::cmp::min(bits, SEL4_MAX_UNTYPED_BITS as u8);
        if (bits as usize) < SEL4_MIN_UNTYPED_BITS {
            // Range too small / misaligned to round; drop the rest.
            self.cursor = self.end;
            return None;
        }
        let base = self.cursor;
        self.cursor += 1u64 << bits;
        Some((base, bits))
    }
}

/// Helper used by boot code: emit an untyped cap for a given chunk.
pub fn make_untyped_cap(base_kva: u64, bits: u8, is_device: bool) -> Cap {
    Cap::new_untyped(base_kva, bits as u64, 0, is_device)
}
