//! Word-packed seL4 ABI types.
//!
//! Layouts pulled from `build-riscv64/libsel4/include/sel4/shared_types_gen.h`.
//! Each helper is `const fn` so we can build BootInfo etc. at compile time.

#![allow(dead_code)]

/// `seL4_Word` on RV64.
pub type Word = u64;

/// `seL4_CPtr`. Capability pointer (index into a CSpace).
pub type CPtr = Word;

/// `seL4_NodeId`.
pub type NodeId = Word;

/// `seL4_Domain`.
pub type Domain = u8;

/// `seL4_SlotPos` — position of a slot in a CNode.
pub type SlotPos = Word;

/// `seL4_MessageInfo` packed word.
///
/// Field layout (bits, low → high):
///   [0..7)   length        (7 bits)
///   [7..9)   extraCaps     (2 bits)
///   [9..12)  capsUnwrapped (3 bits)
///   [12..64) label         (52 bits)
#[repr(transparent)]
#[derive(Copy, Clone, Debug)]
pub struct MessageInfo(pub Word);

impl MessageInfo {
    #[inline]
    pub const fn new(label: Word, caps_unwrapped: Word, extra_caps: Word, length: Word) -> Self {
        let w = ((label & 0x000F_FFFF_FFFF_FFFF) << 12)
            | ((caps_unwrapped & 0x7) << 9)
            | ((extra_caps & 0x3) << 7)
            | (length & 0x7F);
        MessageInfo(w)
    }

    #[inline] pub const fn label(self) -> Word { (self.0 >> 12) & 0x000F_FFFF_FFFF_FFFF }
    #[inline] pub const fn caps_unwrapped(self) -> Word { (self.0 >> 9) & 0x7 }
    #[inline] pub const fn extra_caps(self) -> Word { (self.0 >> 7) & 0x3 }
    #[inline] pub const fn length(self) -> Word { self.0 & 0x7F }
}

/// `seL4_CapRights` packed word.
///
/// Field layout (bits, low → high):
///   [0..1) capAllowWrite
///   [1..2) capAllowRead
///   [2..3) capAllowGrant
///   [3..4) capAllowGrantReply
#[repr(transparent)]
#[derive(Copy, Clone, Debug)]
pub struct CapRights(pub Word);

impl CapRights {
    #[inline]
    pub const fn new(grant_reply: bool, grant: bool, read: bool, write: bool) -> Self {
        CapRights(
            ((grant_reply as Word) << 3)
                | ((grant as Word) << 2)
                | ((read as Word) << 1)
                | (write as Word),
        )
    }
    pub const ALL_RIGHTS: Self = Self::new(true, true, true, true);
    pub const NULL_RIGHTS: Self = Self::new(false, false, false, false);
}

/// `seL4_CNode_CapData` packed word.
///
/// Field layout:
///   [0..6)  guardSize (6 bits)
///   [6..64) guard     (58 bits)
#[repr(transparent)]
#[derive(Copy, Clone, Debug)]
pub struct CNodeCapData(pub Word);

impl CNodeCapData {
    #[inline]
    pub const fn new(guard: Word, guard_size: Word) -> Self {
        CNodeCapData(
            ((guard & 0x03FF_FFFF_FFFF_FFFF) << 6)
                | (guard_size & 0x3F),
        )
    }
}
