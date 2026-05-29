//! `mdb_node_t`: per-CTE entry of the capability derivation tree (CDT).
//!
//! Layout (mirrors `mdb_node_new`):
//!   words[0]: mdbPrev (full 64 bits — kernel VA of previous CTE)
//!   words[1]: [mdbNext:2..39 (sign-ext, 4-byte aligned)] [mdbRevocable:1]
//!             [mdbFirstBadged:0]

#![allow(dead_code)]

const NEXT_MASK: u64 = 0x7F_FFFF_FFFC;
const SIGN_EXT: u64 = 0xFFFF_FF80_0000_0000;

#[inline]
const fn sign_extend_next(low: u64) -> u64 {
    if (low & (1 << 38)) != 0 {
        low | SIGN_EXT
    } else {
        low
    }
}

#[repr(C)]
#[derive(Copy, Clone, Debug, Default)]
pub struct MdbNode {
    pub words: [u64; 2],
}

impl MdbNode {
    pub const NULL: MdbNode = MdbNode { words: [0, 0] };

    #[inline]
    pub const fn new(prev: u64, next: u64, revocable: bool, first_badged: bool) -> MdbNode {
        let mut m = MdbNode::NULL;
        m.words[0] = prev;
        m.words[1] = (next & NEXT_MASK) | ((revocable as u64) << 1) | (first_badged as u64);
        m
    }

    #[inline]
    pub const fn prev(self) -> u64 {
        self.words[0]
    }

    #[inline]
    pub const fn next(self) -> u64 {
        sign_extend_next(self.words[1] & NEXT_MASK)
    }

    #[inline]
    pub const fn revocable(self) -> bool {
        ((self.words[1] >> 1) & 1) != 0
    }

    #[inline]
    pub const fn first_badged(self) -> bool {
        (self.words[1] & 1) != 0
    }

    pub fn set_prev(&mut self, v: u64) {
        self.words[0] = v;
    }

    pub fn set_next(&mut self, v: u64) {
        self.words[1] &= !NEXT_MASK;
        self.words[1] |= v & NEXT_MASK;
    }

    pub fn set_revocable(&mut self, v: bool) {
        self.words[1] &= !0x2;
        self.words[1] |= (v as u64) << 1;
    }

    pub fn set_first_badged(&mut self, v: bool) {
        self.words[1] &= !0x1;
        self.words[1] |= v as u64;
    }
}

const _: () = assert!(core::mem::size_of::<MdbNode>() == 16);
