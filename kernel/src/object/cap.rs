//! `cap_t`: 2-word packed capability representation.
//!
//! Bit layout per cap kind is reproduced byte-for-byte from
//! `build-riscv64/kernel/generated/arch/object/structures_gen.h`. The
//! `capType` tag lives in `words[0][59..64]`, and pointer fields are
//! sign-extended from bit 38 so they always denote a high-half kernel VA.

#![allow(dead_code)]

/// Sign-extension mask for 39-bit kernel pointers stored in caps. Bit 38
/// is the sign bit; when set, bits 39..64 must be all 1.
const SIGN_EXT_MASK: u64 = 0xFFFF_FF80_0000_0000;
const PTR_LOW_MASK: u64 = 0x7F_FFFF_FFFF; // 39 bits

#[inline]
const fn ptr_low(ptr: u64) -> u64 {
    ptr & PTR_LOW_MASK
}

#[inline]
const fn sign_extend_ptr(low39: u64) -> u64 {
    if (low39 & (1 << 38)) != 0 {
        low39 | SIGN_EXT_MASK
    } else {
        low39
    }
}

/// `cap_tag_t` — note the unusual sparse numbering inherited from the
/// kernel `.bf` generator.
#[repr(u8)]
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum CapTag {
    Null = 0,
    Frame = 1,
    Untyped = 2,
    PageTable = 3,
    Endpoint = 4,
    Notification = 6,
    Reply = 8,
    CNode = 10,
    AsidControl = 11,
    Thread = 12,
    AsidPool = 13,
    IrqControl = 14,
    IrqHandler = 16,
    Zombie = 18,
    Domain = 20,
}

impl CapTag {
    pub fn from_u64(v: u64) -> Option<Self> {
        Some(match v {
            0 => Self::Null,
            1 => Self::Frame,
            2 => Self::Untyped,
            3 => Self::PageTable,
            4 => Self::Endpoint,
            6 => Self::Notification,
            8 => Self::Reply,
            10 => Self::CNode,
            11 => Self::AsidControl,
            12 => Self::Thread,
            13 => Self::AsidPool,
            14 => Self::IrqControl,
            16 => Self::IrqHandler,
            18 => Self::Zombie,
            20 => Self::Domain,
            _ => return None,
        })
    }
}

/// Packed 2-word capability.
#[repr(C)]
#[derive(Copy, Clone, Debug, Default)]
pub struct Cap {
    pub words: [u64; 2],
}

impl Cap {
    #[inline]
    pub const fn null() -> Cap {
        Cap { words: [0, 0] }
    }

    #[inline]
    pub const fn tag_raw(self) -> u64 {
        (self.words[0] >> 59) & 0x1F
    }

    #[inline]
    pub fn tag(self) -> Option<CapTag> {
        CapTag::from_u64(self.tag_raw())
    }

    #[inline]
    pub fn is_null(self) -> bool {
        self.tag_raw() == CapTag::Null as u64
    }

    // ---- Untyped cap ------------------------------------------------------
    //
    // words[0]: [tag:59..64] [capPtr:0..39 (sign-ext)]
    // words[1]: [capFreeIndex:25..64] [capIsDevice:6] [capBlockSize:0..6]
    //
    // capPtr  = kernel-window VA of the start of the untyped region
    // capBlockSize = log2 of the region in bytes
    // capFreeIndex = offset (>> seL4_MinUntypedBits=4) of the next free byte
    // capIsDevice  = 1 for MMIO-backed untyped, 0 for regular RAM

    #[inline]
    pub const fn new_untyped(
        ptr: u64,
        block_size_bits: u64,
        free_index: u64,
        is_device: bool,
    ) -> Cap {
        let mut c = Cap::null();
        c.words[0] = ((CapTag::Untyped as u64) << 59) | ptr_low(ptr);
        c.words[1] = ((free_index & PTR_LOW_MASK) << 25)
            | (((is_device as u64) & 0x1) << 6)
            | (block_size_bits & 0x3F);
        c
    }

    #[inline]
    pub const fn untyped_ptr(self) -> u64 {
        sign_extend_ptr(self.words[0] & PTR_LOW_MASK)
    }

    #[inline]
    pub const fn untyped_block_size_bits(self) -> u64 {
        self.words[1] & 0x3F
    }

    #[inline]
    pub const fn untyped_is_device(self) -> bool {
        ((self.words[1] >> 6) & 0x1) != 0
    }

    #[inline]
    pub const fn untyped_free_index(self) -> u64 {
        (self.words[1] >> 25) & PTR_LOW_MASK
    }

    pub fn set_untyped_free_index(&mut self, v: u64) {
        self.words[1] &= !(PTR_LOW_MASK << 25);
        self.words[1] |= (v & PTR_LOW_MASK) << 25;
    }

    // ---- CNode cap --------------------------------------------------------
    //
    // words[0]: [tag:59..64] [guardSize:53..59] [radix:47..53]
    //           [capPtr>>1: 0..39 sign-ext]
    // words[1]: [guard:0..64]
    //
    // capPtr is the kernel-window VA of the CNode's contiguous CTE array.
    // It must be 16-byte aligned (lowest bit always 0, so it's stored
    // shifted right by 1).

    #[inline]
    pub const fn new_cnode(ptr: u64, radix: u64, guard: u64, guard_size: u64) -> Cap {
        let mut c = Cap::null();
        c.words[0] = ((CapTag::CNode as u64) << 59)
            | ((guard_size & 0x3F) << 53)
            | ((radix & 0x3F) << 47)
            | ((ptr_low(ptr) >> 1) & 0x3F_FFFF_FFFF);
        c.words[1] = guard;
        c
    }

    #[inline]
    pub const fn cnode_radix(self) -> u64 {
        (self.words[0] >> 47) & 0x3F
    }

    #[inline]
    pub const fn cnode_guard_size(self) -> u64 {
        (self.words[0] >> 53) & 0x3F
    }

    #[inline]
    pub const fn cnode_guard(self) -> u64 {
        self.words[1]
    }

    #[inline]
    pub const fn cnode_ptr(self) -> u64 {
        // Stored as (ptr >> 1) in the low 39 bits; restore via << 1.
        sign_extend_ptr((self.words[0] & 0x3F_FFFF_FFFF) << 1)
    }

    // ---- Thread (TCB) cap -------------------------------------------------
    //
    // words[0]: [tag:59..64] [capTCBPtr:0..39 (sign-ext)]
    // words[1]: 0

    #[inline]
    pub const fn new_thread(ptr: u64) -> Cap {
        let mut c = Cap::null();
        c.words[0] = ((CapTag::Thread as u64) << 59) | ptr_low(ptr);
        c
    }

    #[inline]
    pub const fn thread_ptr(self) -> u64 {
        sign_extend_ptr(self.words[0] & PTR_LOW_MASK)
    }

    // ---- Domain / IRQControl caps -----------------------------------------

    #[inline]
    pub const fn new_domain() -> Cap {
        let mut c = Cap::null();
        c.words[0] = (CapTag::Domain as u64) << 59;
        c
    }

    #[inline]
    pub const fn new_irq_control() -> Cap {
        let mut c = Cap::null();
        c.words[0] = (CapTag::IrqControl as u64) << 59;
        c
    }

    // ---- Frame cap (RISC-V 4K/Mega/Giga) ----------------------------------
    //
    // words[0]: [tag:59..64] [capFSize:57..59] [capFVMRights:55..57]
    //           [capFIsDevice:54] [capFMappedAddress:0..39]
    // words[1]: [capFMappedASID:48..64] [capFBasePtr:9..48]
    //
    // capFSize: 0 = 4 KiB, 1 = 2 MiB, 2 = 1 GiB (RISCV_4K/Mega/Giga_Page)
    // capFVMRights: 0=NoAccess 1=Read 2=ReadWrite 3=Read
    // (matches `wordFromVMRights` in the C kernel)

    #[inline]
    pub const fn new_frame(base_ptr: u64, size_class: u64, rights: u64, is_device: bool) -> Cap {
        let mut c = Cap::null();
        c.words[0] = ((CapTag::Frame as u64) << 59)
            | ((size_class & 0x3) << 57)
            | ((rights & 0x3) << 55)
            | (((is_device as u64) & 0x1) << 54);
        c.words[1] = (base_ptr & PTR_LOW_MASK) << 9;
        c
    }

    #[inline]
    pub const fn frame_base_ptr(self) -> u64 {
        sign_extend_ptr((self.words[1] >> 9) & PTR_LOW_MASK)
    }

    // ---- Page Table cap ---------------------------------------------------
    //
    // words[0]: [tag:59..64] [capPTIsMapped:39] [capPTMappedAddress:0..39]
    // words[1]: [capPTMappedASID:48..64] [capPTBasePtr:9..48]

    #[inline]
    pub const fn new_page_table(base_ptr: u64) -> Cap {
        let mut c = Cap::null();
        c.words[0] = (CapTag::PageTable as u64) << 59;
        c.words[1] = (base_ptr & PTR_LOW_MASK) << 9;
        c
    }

    #[inline]
    pub const fn page_table_base_ptr(self) -> u64 {
        sign_extend_ptr((self.words[1] >> 9) & PTR_LOW_MASK)
    }

    // ---- Endpoint / Notification (placeholders, will fill in M3.3) -------

    #[inline]
    pub const fn new_endpoint(ptr: u64) -> Cap {
        let mut c = Cap::null();
        c.words[0] = ((CapTag::Endpoint as u64) << 59) | ptr_low(ptr);
        c
    }

    #[inline]
    pub const fn new_notification(ptr: u64) -> Cap {
        let mut c = Cap::null();
        c.words[0] = ((CapTag::Notification as u64) << 59) | ptr_low(ptr);
        c
    }
}

const _: () = {
    assert!(core::mem::size_of::<Cap>() == 16);
    assert!(core::mem::align_of::<Cap>() == 8);
};

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn null_tag() {
        let c = Cap::null();
        assert_eq!(c.tag(), Some(CapTag::Null));
    }

    #[test]
    fn untyped_roundtrip() {
        let c = Cap::new_untyped(0xFFFF_FFFF_8021_0000, 12, 0, false);
        assert_eq!(c.tag(), Some(CapTag::Untyped));
        assert_eq!(c.untyped_block_size_bits(), 12);
        assert_eq!(c.untyped_ptr(), 0xFFFF_FFFF_8021_0000);
    }
}
