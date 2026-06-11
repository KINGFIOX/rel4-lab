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
const PTR48_LOW_MASK: u64 = 0x0000_FFFF_FFFF_FFFF;
const ZOMBIE_TYPE_TCB: u64 = 1 << 6;
const TCB_CNODE_RADIX: u64 = 4;

pub(crate) const FRAME_SIZE_4K: u64 = 0;
pub(crate) const FRAME_SIZE_MEGAPAGE: u64 = 1;
pub(crate) const FRAME_SIZE_GIGAPAGE: u64 = 2;

pub(crate) const FRAME_RIGHTS_KERNEL_ONLY: u64 = 1;
pub(crate) const FRAME_RIGHTS_READ_ONLY: u64 = 2;
pub(crate) const FRAME_RIGHTS_READ_WRITE: u64 = 3;

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

#[inline]
const fn sign_extend_ptr48(low48: u64) -> u64 {
    if (low48 & (1 << 47)) != 0 {
        low48 | 0xFFFF_0000_0000_0000
    } else {
        low48
    }
}

#[inline]
const fn low_mask(bits: u64) -> u64 {
    if bits >= 64 {
        u64::MAX
    } else {
        (1u64 << bits) - 1
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
    SchedContext = 22,
    SchedControl = 24,
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
            22 => Self::SchedContext,
            24 => Self::SchedControl,
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
    #[inline]
    pub const fn new_sched_control(core: u64) -> Cap {
        let mut c = Cap::null();
        c.words[0] = (CapTag::SchedControl as u64) << 59;
        c.words[1] = core;
        c
    }

    // ---- Zombie cap ------------------------------------------------------
    //
    // Matches seL4 `Zombie_new(number, type, ptr)`: capZombieType lives in
    // words[0][0..7], and capZombieID packs the aligned CTE pointer with
    // the remaining slot count in the low `radix + 1` bits.

    #[inline]
    pub const fn new_cnode_zombie(number: u64, radix: u64, ptr: u64) -> Cap {
        let mask = low_mask(radix + 1);
        let mut c = Cap::null();
        c.words[0] = ((CapTag::Zombie as u64) << 59) | (radix & 0x7F);
        c.words[1] = (ptr & !mask) | (number & mask);
        c
    }

    #[inline]
    pub const fn new_tcb_zombie(number: u64, ptr: u64) -> Cap {
        let mask = low_mask(TCB_CNODE_RADIX + 1);
        let mut c = Cap::null();
        c.words[0] = ((CapTag::Zombie as u64) << 59) | ZOMBIE_TYPE_TCB;
        c.words[1] = (ptr & !mask) | (number & mask);
        c
    }

    #[inline]
    pub const fn zombie_id(self) -> u64 {
        self.words[1]
    }

    #[inline]
    pub const fn zombie_type(self) -> u64 {
        self.words[0] & 0x7F
    }

    #[inline]
    pub const fn zombie_is_tcb(self) -> bool {
        self.zombie_type() == ZOMBIE_TYPE_TCB
    }

    #[inline]
    pub const fn zombie_bits(self) -> u64 {
        if self.zombie_is_tcb() {
            TCB_CNODE_RADIX
        } else {
            self.zombie_type()
        }
    }

    #[inline]
    pub const fn zombie_number(self) -> u64 {
        self.zombie_id() & low_mask(self.zombie_bits() + 1)
    }

    #[inline]
    pub const fn zombie_ptr(self) -> u64 {
        self.zombie_id() & !low_mask(self.zombie_bits() + 1)
    }

    #[inline]
    pub fn set_zombie_number(&mut self, number: u64) {
        let mask = low_mask(self.zombie_bits() + 1);
        self.words[1] = (self.words[1] & !mask) | (number & mask);
    }

    #[inline]
    pub const fn new_irq_handler(irq: u64) -> Cap {
        let mut c = Cap::null();
        c.words[0] = (CapTag::IrqHandler as u64) << 59;
        c.words[1] = irq & 0xFFF;
        c
    }

    #[inline]
    pub const fn irq_handler_irq(self) -> u64 {
        self.words[1] & 0xFFF
    }

    #[inline]
    pub const fn new_asid_control() -> Cap {
        let mut c = Cap::null();
        c.words[0] = (CapTag::AsidControl as u64) << 59;
        c
    }

    #[inline]
    pub const fn new_asid_pool(base: u64, ptr: u64) -> Cap {
        // words[0]: [tag:59..64] [capASIDBase:43..59] [capASIDPool:0..38, shifted left by 2]
        let mut c = Cap::null();
        c.words[0] = ((CapTag::AsidPool as u64) << 59)
            | ((base & 0xFFFF) << 43)
            | ((ptr_low(ptr) >> 2) & 0x1F_FFFF_FFFF);
        c
    }

    #[inline]
    pub const fn asid_pool_base(self) -> u16 {
        ((self.words[0] >> 43) & 0xFFFF) as u16
    }

    #[inline]
    pub const fn asid_pool_ptr(self) -> u64 {
        sign_extend_ptr((self.words[0] & 0x1F_FFFF_FFFF) << 2)
    }

    // ---- Frame cap (RISC-V 4K/Mega/Giga) ----------------------------------
    //
    // words[0]: [tag:59..64] [capFSize:57..59] [capFVMRights:55..57]
    //           [capFIsDevice:54] [capFMappedAddress:0..39]
    // words[1]: [capFMappedASID:48..64] [capFBasePtr:9..48]
    //
    // capFSize: 0 = 4 KiB, 1 = 2 MiB, 2 = 1 GiB (RISCV_4K/Mega/Giga_Page)
    // capFVMRights matches RISC-V `wordFromVMRights`:
    // 1 = VMKernelOnly, 2 = VMReadOnly, 3 = VMReadWrite.

    #[inline]
    pub const fn new_frame(base_ptr: u64, size_class: u64, rights: u64, is_device: bool) -> Cap {
        let mut c = Cap::null();
        c.words[0] = ((CapTag::Frame as u64) << 59)
            | ((size_class & 0x3) << 57)
            | ((rights & 0x3) << 55)
            | (((is_device as u64) & 0x1) << 54);
        // words[1]: [capFMappedASID:48..64] [capFBasePtr:9..48]
        c.words[1] = (base_ptr & PTR_LOW_MASK) << 9;
        c
    }

    /// `capFMappedASID` — the ASID (vspace identifier) of the VSpace the
    /// frame is currently mapped into. 0 == not currently mapped.
    #[inline]
    pub const fn frame_mapped_asid(self) -> u16 {
        ((self.words[1] >> 48) & 0xFFFF) as u16
    }

    pub fn set_frame_mapped_asid(&mut self, asid: u16) {
        self.words[1] &= !(0xFFFFu64 << 48);
        self.words[1] |= ((asid as u64) & 0xFFFF) << 48;
    }

    #[inline]
    pub const fn frame_base_ptr(self) -> u64 {
        sign_extend_ptr((self.words[1] >> 9) & PTR_LOW_MASK)
    }

    /// `capFMappedAddress` — the user-space VA the frame is currently
    /// mapped at. `capFMappedASID == 0` is the unmapped marker, so VA 0
    /// remains a valid recorded mapping address.
    #[inline]
    pub const fn frame_mapped_addr(self) -> u64 {
        sign_extend_ptr(self.words[0] & PTR_LOW_MASK)
    }

    pub fn set_frame_mapped_addr(&mut self, addr: u64) {
        self.words[0] &= !PTR_LOW_MASK;
        self.words[0] |= addr & PTR_LOW_MASK;
    }

    #[inline]
    pub const fn frame_is_mapped(self) -> bool {
        self.frame_mapped_asid() != 0
    }

    #[inline]
    pub const fn frame_vm_rights(self) -> u64 {
        (self.words[0] >> 55) & 0x3
    }

    #[inline]
    pub fn set_frame_vm_rights(&mut self, rights: u64) {
        self.words[0] &= !(0x3u64 << 55);
        self.words[0] |= (rights & 0x3) << 55;
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
    pub fn set_page_table_mapping(&mut self, asid: u16, mapped_addr: u64) {
        self.words[0] &= !(PTR_LOW_MASK | (1u64 << 39));
        self.words[0] |= (mapped_addr & PTR_LOW_MASK) | (1u64 << 39);
        self.words[1] &= !(0xFFFFu64 << 48);
        self.words[1] |= ((asid as u64) & 0xFFFF) << 48;
    }

    #[inline]
    pub const fn page_table_mapped_addr(self) -> u64 {
        sign_extend_ptr(self.words[0] & PTR_LOW_MASK)
    }

    #[inline]
    pub const fn page_table_mapped_asid(self) -> u16 {
        ((self.words[1] >> 48) & 0xFFFF) as u16
    }

    #[inline]
    pub const fn page_table_is_mapped(self) -> bool {
        ((self.words[0] >> 39) & 0x1) != 0
    }

    #[inline]
    pub fn clear_page_table_mapping(&mut self) {
        self.words[0] &= !(PTR_LOW_MASK | (1u64 << 39));
        self.words[1] &= !(0xFFFFu64 << 48);
    }

    #[inline]
    pub fn clear_page_table_is_mapped(&mut self) {
        self.words[0] &= !(1u64 << 39);
    }

    #[inline]
    pub const fn page_table_base_ptr(self) -> u64 {
        sign_extend_ptr((self.words[1] >> 9) & PTR_LOW_MASK)
    }

    // ---- Endpoint / Notification ---------------------------------------
    //
    // Layout mirrors `cap_endpoint_cap_new` / `cap_notification_cap_new`
    // from `kernel/generated/arch/object/structures_gen.h`:
    //
    //   Endpoint words[0]:
    //     [59..64) capType
    //     [58]     capCanGrantReply
    //     [57]     capCanGrant
    //     [56]     capCanReceive
    //     [55]     capCanSend
    //     [0..39)  capEPPtr (sign-extended)
    //   Endpoint words[1]: capEPBadge
    //
    //   Notification words[0]:
    //     [59..64) capType
    //     [58]     capNtfnCanReceive
    //     [57]     capNtfnCanSend
    //     [0..39)  capNtfnPtr (sign-extended)
    //   Notification words[1]: capNtfnBadge

    #[inline]
    pub const fn new_endpoint(ptr: u64) -> Cap {
        let mut c = Cap::null();
        c.words[0] = ((CapTag::Endpoint as u64) << 59)
            // CanGrantReply | CanGrant | CanReceive | CanSend
            | (1u64 << 58)
            | (1u64 << 57)
            | (1u64 << 56)
            | (1u64 << 55)
            | ptr_low(ptr);
        c.words[1] = 0;
        c
    }

    #[inline]
    pub const fn new_notification(ptr: u64) -> Cap {
        let mut c = Cap::null();
        c.words[0] = ((CapTag::Notification as u64) << 59)
            // CanReceive | CanSend
            | (1u64 << 58)
            | (1u64 << 57)
            | ptr_low(ptr);
        c.words[1] = 0;
        c
    }

    #[inline]
    pub const fn new_reply_object(reply_ptr: u64, can_grant: bool) -> Cap {
        let mut c = Cap::null();
        c.words[0] = ((CapTag::Reply as u64) << 59) | (((can_grant as u64) & 0x1) << 58);
        c.words[1] = reply_ptr;
        c
    }
    #[inline]
    pub const fn new_sched_context(ptr: u64, size_bits: u64) -> Cap {
        let mut c = Cap::null();
        c.words[0] = (CapTag::SchedContext as u64) << 59;
        c.words[1] = ((ptr & PTR_LOW_MASK) << 16) | ((size_bits & 0x3F) << 10);
        c
    }

    #[inline]
    pub const fn endpoint_ptr(self) -> u64 {
        sign_extend_ptr(self.words[0] & PTR_LOW_MASK)
    }

    #[inline]
    pub const fn endpoint_badge(self) -> u64 {
        self.words[1]
    }

    pub fn set_endpoint_badge(&mut self, badge: u64) {
        self.words[1] = badge;
    }

    #[inline]
    pub const fn endpoint_can_send(self) -> bool {
        (self.words[0] >> 55) & 1 != 0
    }

    #[inline]
    pub const fn endpoint_can_receive(self) -> bool {
        (self.words[0] >> 56) & 1 != 0
    }

    #[inline]
    pub const fn endpoint_can_grant(self) -> bool {
        (self.words[0] >> 57) & 1 != 0
    }

    #[inline]
    pub const fn endpoint_can_grant_reply(self) -> bool {
        (self.words[0] >> 58) & 1 != 0
    }

    #[inline]
    pub const fn reply_is_object(self) -> bool {
        self.tag_raw() == CapTag::Reply as u64 && self.words[1] != 0
    }
    #[inline]
    pub const fn reply_object_ptr(self) -> u64 {
        self.words[1]
    }
    #[inline]
    pub const fn reply_object_can_grant(self) -> bool {
        (self.words[0] >> 58) & 1 != 0
    }
    #[inline]
    pub fn set_reply_object_can_grant(&mut self, can_grant: bool) {
        self.words[0] &= !(1u64 << 58);
        self.words[0] |= ((can_grant as u64) & 0x1) << 58;
    }
    #[inline]
    pub const fn sched_context_ptr(self) -> u64 {
        sign_extend_ptr((self.words[1] >> 16) & PTR_LOW_MASK)
    }
    #[inline]
    pub const fn sched_context_size_bits(self) -> u64 {
        (self.words[1] >> 10) & 0x3F
    }
    #[inline]
    pub const fn sched_control_core(self) -> u64 {
        self.words[1]
    }

    #[inline]
    pub const fn notification_ptr(self) -> u64 {
        sign_extend_ptr(self.words[0] & PTR_LOW_MASK)
    }

    #[inline]
    pub const fn notification_badge(self) -> u64 {
        self.words[1]
    }

    pub fn set_notification_badge(&mut self, badge: u64) {
        self.words[1] = badge;
    }

    #[inline]
    pub const fn notification_can_send(self) -> bool {
        (self.words[0] >> 57) & 1 != 0
    }

    #[inline]
    pub const fn notification_can_receive(self) -> bool {
        (self.words[0] >> 58) & 1 != 0
    }

    #[inline]
    pub const fn frame_size(self) -> u64 {
        (self.words[0] >> 57) & 0x3
    }

    #[inline]
    pub const fn frame_is_device(self) -> bool {
        ((self.words[0] >> 54) & 0x1) != 0
    }
}

const _: () = {
    assert!(core::mem::size_of::<Cap>() == 16);
    assert!(core::mem::align_of::<Cap>() == 8);
};
