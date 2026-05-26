//! `seL4_BootInfo` and friends, byte-exact mirror of
//! `kernel/libsel4/include/sel4/bootinfo_types.h` for our non-MCS build.

#![allow(dead_code)]

use super::constants::MAX_NUM_BOOTINFO_UNTYPED_CAPS;
use super::types::{Domain, NodeId, SlotPos, Word};

// Fixed root CNode slot positions (must match `enum seL4_RootCNodeCapSlots`).
pub const CAP_NULL: SlotPos = 0;
pub const CAP_INIT_THREAD_TCB: SlotPos = 1;
pub const CAP_INIT_THREAD_CNODE: SlotPos = 2;
pub const CAP_INIT_THREAD_VSPACE: SlotPos = 3;
pub const CAP_IRQ_CONTROL: SlotPos = 4;
pub const CAP_ASID_CONTROL: SlotPos = 5;
pub const CAP_INIT_THREAD_ASID_POOL: SlotPos = 6;
pub const CAP_IO_PORT_CONTROL: SlotPos = 7;
pub const CAP_IO_SPACE: SlotPos = 8;
pub const CAP_BOOT_INFO_FRAME: SlotPos = 9;
pub const CAP_INIT_THREAD_IPC_BUFFER: SlotPos = 10;
pub const CAP_DOMAIN: SlotPos = 11;
pub const CAP_SMMU_SID_CONTROL: SlotPos = 12;
pub const CAP_SMMU_CB_CONTROL: SlotPos = 13;
pub const CAP_INIT_THREAD_SC: SlotPos = 14;
pub const CAP_SMC: SlotPos = 15;
pub const NUM_INITIAL_CAPS: SlotPos = 16;

#[repr(C)]
#[derive(Copy, Clone, Debug, Default)]
pub struct SlotRegion {
    pub start: SlotPos,
    pub end: SlotPos,
}

#[repr(C)]
#[derive(Copy, Clone, Debug, Default)]
pub struct UntypedDesc {
    pub paddr: Word,
    pub size_bits: u8,
    pub is_device: u8,
    pub _padding: [u8; 6],
}

const _: () = {
    assert!(core::mem::size_of::<UntypedDesc>() == 16);
};

/// `seL4_BootInfo` for non-MCS build. Field order is load-bearing.
#[repr(C)]
pub struct BootInfo {
    pub extra_len: Word,
    pub node_id: NodeId,
    pub num_nodes: Word,
    pub num_io_pt_levels: Word,
    /// User-VA pointer to `seL4_IPCBuffer`.
    pub ipc_buffer: Word,
    pub empty: SlotRegion,
    pub shared_frames: SlotRegion,
    pub user_image_frames: SlotRegion,
    pub user_image_paging: SlotRegion,
    pub io_space_caps: SlotRegion,
    pub extra_bi_pages: SlotRegion,
    pub init_thread_cnode_size_bits: Word,
    pub init_thread_domain: Domain,
    pub _pad_domain: [u8; 7],
    // MCS-only field would go here; omitted for our non-MCS build.
    pub untyped: SlotRegion,
    pub untyped_list: [UntypedDesc; MAX_NUM_BOOTINFO_UNTYPED_CAPS],
}

const _: () = {
    // BootInfo MUST fit in one 4 KiB page.
    assert!(core::mem::size_of::<BootInfo>() <= 4096);
};

/// `seL4_IPCBuffer`. Layout from
/// `kernel/libsel4/include/sel4/shared_types.bf`:
///
/// ```c
/// typedef struct seL4_IPCBuffer {
///     seL4_MessageInfo_t tag;
///     seL4_Word msg[seL4_MsgMaxLength]; // 120
///     seL4_Word userData;
///     seL4_CPtr caps_or_badges[seL4_MsgMaxExtraCaps]; // 3
///     seL4_CPtr receiveCNode;
///     seL4_CPtr receiveIndex;
///     seL4_Word receiveDepth;
/// } seL4_IPCBuffer;
/// ```
#[repr(C)]
pub struct IPCBuffer {
    pub tag: Word,             // packed seL4_MessageInfo
    pub msg: [Word; 120],      // seL4_MsgMaxLength
    pub user_data: Word,
    pub caps_or_badges: [Word; 3],
    pub receive_cnode: Word,
    pub receive_index: Word,
    pub receive_depth: Word,
}

const _: () = {
    // sizeof(seL4_IPCBuffer) on RV64: (1 + 120 + 1 + 3 + 1 + 1 + 1) * 8 = 1024
    assert!(core::mem::size_of::<IPCBuffer>() == 128 * 8);
};
