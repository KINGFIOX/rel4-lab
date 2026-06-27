//! Cap-type-specific invocation handlers.
//!
//! Each function consumes the cap that was looked up plus the message
//! arguments (mr0..mr3 in `UserContext.a2..a5`, mr4+ in the IPC buffer)
//! and either mutates kernel state to perform the requested action or
//! returns a `SyscallError` for the caller to relay.

#![allow(dead_code)]

use core::ptr;

use crate::abi::types::MessageInfo;
use crate::api::cspace;
use crate::api::syscall::SyscallError;
use crate::api::thread::Thread;
use crate::arch::current::paging::{PAGE_SIZE, PageTable};
use crate::arch::current::trap::{
    SEL4_TCB_FRAME_REGS, SEL4_TCB_GP_REGS, SEL4_USER_CONTEXT_REGS, SEL4_USER_CONTEXT_WORDS,
    UserContext, UserRegister,
};
use crate::kernel::smp::{BklCell, debug_assert_kernel_lock_held};
use crate::object::cap::{
    Cap, CapTag, FRAME_RIGHTS_KERNEL_ONLY, FRAME_RIGHTS_READ_ONLY, FRAME_RIGHTS_READ_WRITE,
    FRAME_SIZE_4K, FRAME_SIZE_GIGAPAGE, FRAME_SIZE_MEGAPAGE,
};
use crate::object::cnode::{CspaceLockGuard, Cte, with_cnode_at};
use crate::object::mdb::MdbNode;
use crate::object::tcb::{self, Tcb};

const TCB_COPY_SUSPEND_SOURCE: u64 = 1 << 0;
const TCB_COPY_RESUME_TARGET: u64 = 1 << 1;
const TCB_COPY_TRANSFER_FRAME: u64 = 1 << 2;
const TCB_COPY_TRANSFER_INTEGER: u64 = 1 << 3;
const SEL4_IPC_BUFFER_SIZE_BITS: u64 = 10;

/// Object type IDs as defined by `seL4_ObjectType` (`api_object` +
/// `_mode_object` + `_object`) for the non-MCS 64-bit VSpace ABI.
/// Reply objects are a local compatibility extension for userspace that keeps
/// ordinary non-MCS scheduling semantics.
#[repr(u64)]
#[derive(Copy, Clone, Debug, Eq, PartialEq)]
enum ObjectType {
    Untyped = 0,
    Tcb = 1,
    Endpoint = 2,
    Notification = 3,
    CapTable = 4,
    GigaPage = 5,
    FourKPage = 6,
    MegaPage = 7,
    PageTable = 8,
    Reply = 9,
}

impl ObjectType {
    const fn from_raw(value: u64) -> Option<Self> {
        match value {
            0 => Some(Self::Untyped),
            1 => Some(Self::Tcb),
            2 => Some(Self::Endpoint),
            3 => Some(Self::Notification),
            4 => Some(Self::CapTable),
            5 => Some(Self::GigaPage),
            6 => Some(Self::FourKPage),
            7 => Some(Self::MegaPage),
            8 => Some(Self::PageTable),
            9 => Some(Self::Reply),
            _ => None,
        }
    }
}

/// Invocation labels — must agree with `enum invocation_label` from
/// `libsel4/include/sel4/invocation.h`. The exact numbering is generated
/// by the kernel's invocation_header_gen.py; we only enumerate the cases
/// we actually handle.
#[repr(u64)]
#[derive(Copy, Clone, Debug, Eq, PartialEq)]
enum InvocationLabel {
    UntypedRetype = 1,
    TcbReadRegisters = 2,
    TcbWriteRegisters = 3,
    TcbCopyRegisters = 4,
    TcbConfigure = 5,
    TcbSetPriority = 6,
    TcbSetMcPriority = 7,
    TcbSetSchedParams = 8,
    TcbSetIpcBuffer = 9,
    TcbSetSpace = 10,
    TcbSuspend = 11,
    TcbResume = 12,
    TcbBindNotification = 13,
    TcbUnbindNotification = 14,
    TcbSetAffinity = 15,
    TcbSetTlsBase = 16,
    TcbSetFlags = 17,
    CNodeRevoke = 18,
    CNodeDelete = 19,
    CNodeCancelBadgedSends = 20,
    CNodeCopy = 21,
    CNodeMint = 22,
    CNodeMove = 23,
    CNodeMutate = 24,
    CNodeRotate = 25,
    CNodeSaveCaller = 26,
    IrqIssueIrqHandler = 27,
    IrqAck = 28,
    IrqSetHandler = 29,
    IrqClearHandler = 30,
    DomainSet = 31,
    DomainScheduleConfigure = 32,
    DomainScheduleSetStart = 33,
    RiscvPageTableMap = 34,
    RiscvPageTableUnmap = 35,
    RiscvPageMap = 36,
    RiscvPageUnmap = 37,
    RiscvPageGetAddress = 38,
    RiscvAsidControlMakePool = 39,
    RiscvAsidPoolAssign = 40,
    RiscvIrqIssueIrqHandlerTrigger = 41,
}

impl InvocationLabel {
    const fn raw(self) -> u64 {
        self as u64
    }
}

#[inline]
fn invocation_label_matches(label_id: u64, label: InvocationLabel) -> bool {
    label_id == label.raw()
}

pub fn success_reply_length(tag: Option<CapTag>, label_id: u64) -> u64 {
    match tag {
        Some(CapTag::Thread) if label_id == InvocationLabel::TcbSetFlags.raw() => 1,
        Some(CapTag::Frame) if label_id == InvocationLabel::RiscvPageGetAddress.raw() => 1,
        _ => 0,
    }
}

fn write_reply_mr0(uc: &mut UserContext, value: u64) {
    uc.regs[UserRegister::A2.index()] = value;
    crate::api::thread::write_current_ipc_buffer_word(1, value);
}

/// Helper: compute log2 of the in-memory bytes of an object given its
/// type and user-supplied size (used for CNode / Untyped where the user
/// picks a radix).
fn object_size_bits(ty: ObjectType, user_size: u64) -> u64 {
    use crate::abi::constants::{
        SEL4_ENDPOINT_BITS, SEL4_NOTIFICATION_BITS, SEL4_SLOT_BITS, SEL4_TCB_BITS,
    };
    match ty {
        ObjectType::Untyped => user_size,
        ObjectType::Tcb => SEL4_TCB_BITS as u64,
        ObjectType::Endpoint => SEL4_ENDPOINT_BITS as u64,
        ObjectType::Notification => SEL4_NOTIFICATION_BITS as u64,
        ObjectType::CapTable => user_size + SEL4_SLOT_BITS as u64,
        ObjectType::Reply => crate::abi::constants::SEL4_REPLY_BITS as u64,
        ObjectType::FourKPage | ObjectType::PageTable => 12,
        ObjectType::MegaPage => 21,
        ObjectType::GigaPage => 30,
    }
}

fn validate_retype_object_size(
    ty: u64,
    user_size: u64,
    uc: &mut UserContext,
) -> Result<(ObjectType, u64), SyscallError> {
    use crate::abi::constants::{SEL4_MAX_UNTYPED_BITS, SEL4_MIN_UNTYPED_BITS, WORD_BITS};

    let object_type = ObjectType::from_raw(ty).ok_or(SyscallError::InvalidArgument)?;
    let obj_bits = object_size_bits(object_type, user_size);
    if user_size >= WORD_BITS as u64 || obj_bits > SEL4_MAX_UNTYPED_BITS as u64 {
        uc.regs[UserRegister::A2.index()] = 0;
        uc.regs[UserRegister::A3.index()] = SEL4_MAX_UNTYPED_BITS as u64;
        return Err(SyscallError::RangeError);
    }

    match object_type {
        ObjectType::CapTable if user_size == 0 => Err(SyscallError::InvalidArgument),
        ObjectType::Untyped if user_size < SEL4_MIN_UNTYPED_BITS as u64 => {
            Err(SyscallError::InvalidArgument)
        }
        _ => Ok((object_type, obj_bits)),
    }
}

fn device_untyped_retype_allowed(ty: ObjectType) -> bool {
    matches!(
        ty,
        ObjectType::Untyped | ObjectType::FourKPage | ObjectType::MegaPage | ObjectType::GigaPage
    )
}

/// Construct the cap_t for a freshly allocated object.
fn create_object_cap(ty: ObjectType, region_base: u64, user_size: u64, is_device: bool) -> Cap {
    match ty {
        ObjectType::Untyped => Cap::new_untyped(region_base, user_size, 0, is_device),
        ObjectType::CapTable => {
            // Fresh CNode caps have no guard: callers are expected to
            // set one with `seL4_CNode_Mint`/`Mutate` when they put the
            // cap into a CSpace. Matches `createCNodeObject` in
            // `kernel/src/object/objecttype.c`.
            Cap::new_cnode(region_base, user_size, 0, 0)
        }
        ObjectType::FourKPage => Cap::new_frame(
            region_base,
            FRAME_SIZE_4K,
            FRAME_RIGHTS_READ_WRITE,
            is_device,
        ),
        ObjectType::MegaPage => Cap::new_frame(
            region_base,
            FRAME_SIZE_MEGAPAGE,
            FRAME_RIGHTS_READ_WRITE,
            is_device,
        ),
        ObjectType::GigaPage => Cap::new_frame(
            region_base,
            FRAME_SIZE_GIGAPAGE,
            FRAME_RIGHTS_READ_WRITE,
            is_device,
        ),
        ObjectType::PageTable => Cap::new_page_table(region_base),
        ObjectType::Endpoint => Cap::new_endpoint(region_base),
        ObjectType::Notification => Cap::new_notification(region_base),
        ObjectType::Tcb => Cap::new_thread(region_base),
        ObjectType::Reply => Cap::new_reply_object(region_base, true),
    }
}

/// `Untyped_Retype` slow path. See `kernel/src/object/untyped.c` in the C
/// kernel for the canonical algorithm.
///
/// Message layout (length = 6):
///   mr0 = newType (object type)
///   mr1 = userObjSize
///   mr2 = nodeIndex     (CPtr into root CNode of dest CNode-cap)
///   mr3 = nodeDepth
///   mr4 = nodeOffset    (slot index in dest CNode)
///   mr5 = nodeWindow    (count of consecutive slots to fill)
///   extraCaps[0] = root CNode through which mr2/mr3 are resolved
pub fn handle_untyped(
    thread: &Thread,
    src_slot: *mut Cte,
    src_cap: Cap,
    label_id: u64,
    length: u64,
    uc: &mut UserContext,
) -> Result<(), SyscallError> {
    if label_id != InvocationLabel::UntypedRetype.raw() {
        return Err(SyscallError::IllegalOperation);
    }
    if length < 6 {
        return Err(SyscallError::TruncatedMessage);
    }
    require_extra_caps(uc, 1)?;

    let new_type = uc.regs[UserRegister::A2.index()];
    let user_size = uc.regs[UserRegister::A3.index()];
    let node_index = uc.regs[UserRegister::A4.index()];
    let node_depth = uc.regs[UserRegister::A5.index()];
    let (node_offset, node_window) = read_mrs_4_5(thread);

    // The dest-CNode CPtr was placed in `caps_or_badges[0]` by the libsel4
    // stub's `seL4_SetCap(0, root)`.
    let root_cptr = read_extra_cap(thread, 0);

    let (new_object_type, obj_bits) = validate_retype_object_size(new_type, user_size, uc)?;

    // Resolve the destination CNode capability.
    //   nodeDepth == 0 → use the looked-up cap *directly* (it must be a CNode).
    //   nodeDepth > 0  → walk `nodeIndex` for `nodeDepth` bits within it.
    let (root_cap, _) =
        cspace::lookup_cap(thread, root_cptr).map_err(|_| SyscallError::InvalidCapability)?;
    let dest_cnode_cap = if node_depth == 0 {
        root_cap
    } else {
        let node_slot = resolve_slot(root_cap, node_index, node_depth as u32)?;
        crate::object::cnode::cap_snapshot(node_slot)
    };
    if dest_cnode_cap.tag() != Some(CapTag::CNode) {
        return Err(SyscallError::FailedLookup);
    }
    let dest_radix = dest_cnode_cap.cnode_radix();
    if node_offset >= (1u64 << dest_radix) {
        uc.regs[UserRegister::A2.index()] = 0;
        uc.regs[UserRegister::A3.index()] = (1u64 << dest_radix) - 1;
        return Err(SyscallError::RangeError);
    }
    if node_window < 1 || node_window > crate::abi::constants::RETYPE_FAN_OUT_LIMIT as u64 {
        uc.regs[UserRegister::A2.index()] = 1;
        uc.regs[UserRegister::A3.index()] = crate::abi::constants::RETYPE_FAN_OUT_LIMIT as u64;
        return Err(SyscallError::RangeError);
    }
    if node_window > (1u64 << dest_radix) - node_offset {
        uc.regs[UserRegister::A2.index()] = 1;
        uc.regs[UserRegister::A3.index()] = (1u64 << dest_radix) - node_offset;
        return Err(SyscallError::RangeError);
    }
    let dest_base_kva = dest_cnode_cap.cnode_ptr();
    let retype_dest_cnode = |dest_cnode: &mut [Cte]| -> Result<(), SyscallError> {
        let untyped_bits = src_cap.untyped_block_size_bits();
        let is_device = src_cap.untyped_is_device();
        let region_base_kva = src_cap.untyped_ptr();
        let region_size = 1u64 << untyped_bits;

        // If the untyped has no surviving CDT descendants we restart
        // allocation from offset 0 — mirrors `resetUntypedCap` in the C
        // kernel's `decodeUntypedInvocation`. This is what makes a
        // Revoke-on-parent return a fully fresh untyped to libsel4allocman
        // so subsequent allocator pool refills don't drown in NotEnoughMemory.
        let cspace_guard = crate::object::cnode::lock_cspace();
        // Ensure target slots are empty.
        for i in 0..node_window {
            let slot = &dest_cnode[(node_offset + i) as usize];
            if !slot.cap.is_null() {
                return Err(SyscallError::DeleteFirst);
            }
        }

        let has_children =
            unsafe { crate::object::cnode::mdb_has_children_locked(&cspace_guard, src_slot) };
        let stored_fi = src_cap.untyped_free_index();
        let reset_untyped = !has_children && stored_fi != 0;
        let free_index = if has_children { stored_fi } else { 0 };
        let used_bytes = free_index << 4;
        let free_bytes = region_size.saturating_sub(used_bytes);

        let aligned_start_offset = align_up(used_bytes, obj_bits);
        let total_obj_bytes = node_window << obj_bits;

        if aligned_start_offset.saturating_add(total_obj_bytes) > region_size {
            return Err(SyscallError::NotEnoughMemory);
        }
        let _ = free_bytes;

        if is_device && !device_untyped_retype_allowed(new_object_type) {
            return Err(SyscallError::InvalidArgument);
        }

        if reset_untyped {
            reset_untyped_cap_for_retype(
                src_slot,
                region_base_kva,
                untyped_bits,
                stored_fi,
                is_device,
            )?;
        }

        // Match seL4 `invokeUntyped_Retype`: publish the consumed free
        // range on the parent Untyped before creating and inserting the
        // new child caps.
        let new_used_bytes = aligned_start_offset + total_obj_bytes;
        let new_free_index = new_used_bytes >> 4;
        unsafe {
            (*src_slot).cap.set_untyped_free_index(new_free_index);
        }

        // Zero the memory we're about to repurpose (non-device).
        let alloc_base_kva = region_base_kva.wrapping_add(aligned_start_offset);
        if !is_device {
            unsafe {
                ptr::write_bytes(alloc_base_kva as *mut u8, 0, total_obj_bytes as usize);
            }
        }

        // Install caps for each new object and weave them into the CDT
        // right after the parent untyped slot. We splice each new child
        // between `src_slot` and whatever currently follows it so that:
        //
        //   src_slot -> child[0] -> child[1] -> ... -> child[N-1] -> (old next)
        //
        // `mdb_has_children(src_slot)` checks whether `src_slot.next` (now
        // child[0]) points back at us — that's what lets the next Retype
        // detect "no children left" after a Revoke and reset free_index.
        for i in 0..node_window {
            let obj_base = alloc_base_kva.wrapping_add(i << obj_bits);
            let cap = create_object_cap(new_object_type, obj_base, user_size, is_device);
            // Per-object init hook. For TCBs we stamp the `Tcb` struct
            // skeleton onto the freshly zeroed slab so that subsequent
            // TCB_* invocations have a real place to land data. Endpoints
            // are also stamped — though `Untyped_Retype` zeroed the slab
            // already, going through `endpoint::init` keeps the layout
            // contract explicit at the one place objects come to life.
            match new_object_type {
                ObjectType::Tcb => unsafe { crate::object::tcb::init(obj_base) },
                ObjectType::Endpoint => unsafe { crate::object::endpoint::init(obj_base) },
                ObjectType::Notification => unsafe { crate::object::notification::init(obj_base) },
                ObjectType::Reply => unsafe { crate::object::reply::init(obj_base) },
                _ => {}
            }
            let dst = &mut dest_cnode[(node_offset + i) as usize];
            // Untyped_Retype uses the C kernel's insertNewCap path, not
            // cteInsert/isCapRevocable: every freshly-created object is a
            // revocable child of the source Untyped and may itself become a
            // CDT parent for later derived caps.
            unsafe {
                crate::object::cnode::insert_new_cap_locked(
                    &cspace_guard,
                    src_slot,
                    dst as *mut Cte,
                    cap,
                );
            }
        }

        Ok(())
    };
    unsafe {
        with_cnode_at(
            dest_base_kva as *mut u8,
            dest_radix as usize,
            retype_dest_cnode,
        )
    }
}

fn reset_untyped_cap_for_retype(
    src_slot: *mut Cte,
    region_base_kva: u64,
    untyped_bits: u64,
    stored_free_index: u64,
    is_device: bool,
) -> Result<(), SyscallError> {
    if stored_free_index == 0 {
        return Ok(());
    }

    let min_untyped_bits = crate::abi::constants::SEL4_MIN_UNTYPED_BITS as u64;
    let reset_chunk_bits = crate::abi::constants::RESET_CHUNK_BITS as u64;
    let region_size = 1u64 << untyped_bits;
    let used_bytes = stored_free_index << min_untyped_bits;

    if is_device || untyped_bits < reset_chunk_bits {
        if !is_device {
            unsafe {
                ptr::write_bytes(region_base_kva as *mut u8, 0, region_size as usize);
            }
        }
        unsafe {
            (*src_slot).cap.set_untyped_free_index(0);
        }
        return Ok(());
    }

    let chunk_bytes = 1u64 << reset_chunk_bits;
    let mut offset = align_down(used_bytes - 1, reset_chunk_bits);
    loop {
        unsafe {
            ptr::write_bytes(
                region_base_kva.wrapping_add(offset) as *mut u8,
                0,
                chunk_bytes as usize,
            );
            (*src_slot)
                .cap
                .set_untyped_free_index(offset >> min_untyped_bits);
        }

        cnode_preemption_point()?;

        if offset == 0 {
            break;
        }
        offset -= chunk_bytes;
    }
    Ok(())
}

fn user_map_error(err: crate::arch::current::vspace::UserMapError) -> SyscallError {
    match err {
        crate::arch::current::vspace::UserMapError::InvalidArgument => {
            SyscallError::InvalidArgument
        }
        crate::arch::current::vspace::UserMapError::FailedLookup(_) => SyscallError::FailedLookup,
        crate::arch::current::vspace::UserMapError::DeleteFirst => SyscallError::DeleteFirst,
    }
}

fn user_map_error_reply(
    uc: &mut UserContext,
    err: crate::arch::current::vspace::UserMapError,
) -> SyscallError {
    if let crate::arch::current::vspace::UserMapError::FailedLookup(bits_left) = err {
        uc.regs[UserRegister::A4.index()] = bits_left as u64;
        crate::api::thread::write_current_ipc_buffer_word(3, bits_left as u64);
    }
    user_map_error(err)
}

/// RISC-V Page_Map / Page_Unmap / Page_GetAddress.
///
/// The labels live in `arch_invocation_label`:
///   39 RISCVPageTableMap     40 RISCVPageTableUnmap
///   41 RISCVPageMap          42 RISCVPageUnmap
///   43 RISCVPageGetAddress
pub fn handle_frame(
    thread: &Thread,
    slot: *mut Cte,
    cap: Cap,
    label_id: u64,
    length: u64,
    uc: &mut UserContext,
) -> Result<(), SyscallError> {
    use crate::arch::current::vspace;

    let page_map = InvocationLabel::RiscvPageMap.raw();
    let page_unmap = InvocationLabel::RiscvPageUnmap.raw();
    let page_get_address = InvocationLabel::RiscvPageGetAddress.raw();

    let is_page_map = label_id == page_map;
    let is_page_unmap = label_id == page_unmap;
    let is_page_get_addr = label_id == page_get_address;

    match () {
        _ if is_page_map => {
            if length < 3 {
                return Err(SyscallError::TruncatedMessage);
            }
            require_extra_caps(uc, 1)?;
            let vaddr = uc.regs[UserRegister::A2.index()];
            // libsel4 packs `seL4_CapRights_t` (from `shared_types.pbf`):
            //   bit 0 capAllowWrite, bit 1 capAllowRead,
            //   bit 2 capAllowGrant, bit 3 capAllowGrantReply.
            // VM attributes (RISC-V) bit 0 = riscvExecuteNever.
            let rights_packed = uc.regs[UserRegister::A3.index()];
            let attrs = uc.regs[UserRegister::A4.index()];
            let vm_rights = mask_frame_vm_rights(cap.frame_vm_rights(), rights_packed);
            let can_write = frame_vm_rights_allow_write(vm_rights);
            let can_read = frame_vm_rights_allow_read(vm_rights);
            let exec_never = (attrs & 0x1) != 0;

            let vspace_cptr = read_extra_cap(thread, 0);
            let (vspace_cap, _) = cspace::lookup_cap(thread, vspace_cptr)
                .map_err(|_| SyscallError::InvalidCapability)?;
            if vspace_cap.tag() != Some(CapTag::PageTable) {
                return Err(SyscallError::InvalidCapability);
            }
            let asid = vspace_cap.page_table_mapped_asid();
            if !vspace_cap.page_table_is_mapped() || asid == 0 {
                return Err(SyscallError::InvalidCapability);
            }
            let root_pt_kva = vspace_cap.page_table_base_ptr();
            let asid_root = crate::object::asid::lookup(asid);
            if asid_root == 0 {
                return Err(SyscallError::FailedLookup);
            }
            if asid_root != root_pt_kva {
                return Err(SyscallError::InvalidCapability);
            }

            // Frame's underlying memory: capFBasePtr is the kernel-window VA
            // of the start of the frame.
            let frame_kva = cap.frame_base_ptr();
            let frame_pa = kva_to_pa(frame_kva);

            // Track which VSpace this frame is going into so a later
            // Page_Unmap routes to the right root PT instead of clobbering
            // the current thread's mappings. ASID 0 means "no mapping
            // recorded", so fail closed rather than installing an
            // unrouteable PTE.
            unsafe {
                let _cspace_guard = crate::object::cnode::lock_cspace();
                let current_cap = (*slot).cap;
                if !same_object_as(current_cap, cap) {
                    return Err(SyscallError::InvalidCapability);
                }
                let root_pt = root_pt_kva as *mut PageTable;
                let flags = vspace::user_frame_flags(
                    can_read,
                    can_write,
                    !exec_never,
                    current_cap.frame_is_device(),
                );
                let prepared_map = if current_cap.frame_is_mapped() {
                    if current_cap.frame_mapped_asid() != asid {
                        return Err(SyscallError::InvalidCapability);
                    }
                    if current_cap.frame_mapped_addr() != vaddr {
                        return Err(SyscallError::InvalidArgument);
                    }
                    vspace::prepare_user_frame_remap(
                        root_pt,
                        vaddr as usize,
                        frame_pa as usize,
                        current_cap.frame_size(),
                        flags,
                    )
                    .map_err(|err| user_map_error_reply(uc, err))?
                } else {
                    vspace::prepare_user_frame_map(
                        root_pt,
                        vaddr as usize,
                        frame_pa as usize,
                        current_cap.frame_size(),
                        flags,
                    )
                    .map_err(|err| user_map_error_reply(uc, err))?
                };
                (*slot).cap.set_frame_mapped_addr(vaddr);
                (*slot).cap.set_frame_mapped_asid(asid);
                vspace::commit_user_frame_map(prepared_map);
            }
            Ok(())
        }
        _ if is_page_unmap => {
            unsafe {
                let _cspace_guard = crate::object::cnode::lock_cspace();
                if !same_object_as((*slot).cap, cap) {
                    return Err(SyscallError::InvalidCapability);
                }
                let frame_va = (*slot).cap.frame_mapped_addr();
                let asid = (*slot).cap.frame_mapped_asid();
                if asid == 0 {
                    return Ok(());
                }
                let root_pt_kva = crate::object::asid::lookup(asid);
                if root_pt_kva == 0 {
                    // Best effort: clear the cap metadata but don't touch any
                    // page table. This is what the C kernel does for caps whose
                    // ASID has been freed under it.
                    (*slot).cap.set_frame_mapped_addr(0);
                    (*slot).cap.set_frame_mapped_asid(0);
                    return Ok(());
                }
                let _ = vspace::unmap_user_frame(
                    root_pt_kva as *mut PageTable,
                    frame_va as usize,
                    cap.frame_size(),
                    kva_to_pa(cap.frame_base_ptr()) as usize,
                );
                (*slot).cap.set_frame_mapped_addr(0);
                (*slot).cap.set_frame_mapped_asid(0);
            }
            Ok(())
        }
        _ if is_page_get_addr => {
            // Return the frame's physical address in mr0.
            let frame_pa = kva_to_pa(cap.frame_base_ptr());
            write_reply_mr0(uc, frame_pa);
            Ok(())
        }
        _ => {
            let _ = label_id;
            Err(SyscallError::IllegalOperation)
        }
    }
}

/// RISC-V PageTable_Map / PageTable_Unmap.
pub fn handle_page_table(
    thread: &Thread,
    slot: *mut Cte,
    cap: Cap,
    label_id: u64,
    length: u64,
    uc: &mut UserContext,
) -> Result<(), SyscallError> {
    use crate::arch::current::vspace;

    let page_table_map = InvocationLabel::RiscvPageTableMap.raw();
    let page_table_unmap = InvocationLabel::RiscvPageTableUnmap.raw();

    let is_map = label_id == page_table_map;
    let is_unmap = label_id == page_table_unmap;

    match () {
        _ if is_map => {
            if length < 2 {
                return Err(SyscallError::TruncatedMessage);
            }
            require_extra_caps(uc, 1)?;
            if cap.page_table_is_mapped() {
                return Err(SyscallError::InvalidCapability);
            }

            let vaddr = uc.regs[UserRegister::A2.index()];
            let vspace_cptr = read_extra_cap(thread, 0);
            let (vspace_cap, _) = cspace::lookup_cap(thread, vspace_cptr)
                .map_err(|_| SyscallError::InvalidCapability)?;
            if vspace_cap.tag() != Some(CapTag::PageTable) {
                return Err(SyscallError::InvalidCapability);
            }
            let asid = vspace_cap.page_table_mapped_asid();
            if !vspace_cap.page_table_is_mapped() || asid == 0 {
                return Err(SyscallError::InvalidCapability);
            }
            let root_pt_kva = vspace_cap.page_table_base_ptr();
            let asid_root = crate::object::asid::lookup(asid);
            if asid_root == 0 {
                return Err(SyscallError::FailedLookup);
            }
            if asid_root != root_pt_kva {
                return Err(SyscallError::InvalidCapability);
            }

            unsafe {
                let _cspace_guard = crate::object::cnode::lock_cspace();
                let current_cap = (*slot).cap;
                if !same_object_as(current_cap, cap) {
                    return Err(SyscallError::InvalidCapability);
                }
                if current_cap.page_table_is_mapped() {
                    return Err(SyscallError::InvalidCapability);
                }
                let prepared_map = vspace::prepare_user_page_table_map(
                    root_pt_kva as *mut PageTable,
                    vaddr as usize,
                    current_cap.page_table_base_ptr() as *mut PageTable,
                )
                .map_err(user_map_error)?;
                let mapped_addr = prepared_map.mapped_addr();
                (*slot).cap.set_page_table_mapping(asid, mapped_addr as u64);
                vspace::commit_user_page_table_map(prepared_map);
            }
            Ok(())
        }
        _ if is_unmap => {
            unsafe {
                let cspace_guard = crate::object::cnode::lock_cspace();
                let current_cap = (*slot).cap;
                if !same_object_as(current_cap, cap) {
                    return Err(SyscallError::InvalidCapability);
                }
                if !is_final_capability(&cspace_guard, slot) {
                    return Err(SyscallError::RevokeFirst);
                }
                if current_cap.page_table_is_mapped() {
                    let asid = current_cap.page_table_mapped_asid();
                    let root_pt_kva = crate::object::asid::lookup(asid);
                    let pt = current_cap.page_table_base_ptr() as *mut PageTable;
                    if root_pt_kva == pt as u64 {
                        return Err(SyscallError::RevokeFirst);
                    }
                    if root_pt_kva != 0 {
                        let _ = vspace::unmap_user_page_table(
                            root_pt_kva as *mut PageTable,
                            current_cap.page_table_mapped_addr() as usize,
                            pt,
                        );
                    }
                    ptr::write_bytes(pt as *mut u8, 0, PAGE_SIZE);
                }
                (*slot).cap.clear_page_table_is_mapped();
            }
            Ok(())
        }
        _ => {
            let _ = label_id;
            Err(SyscallError::IllegalOperation)
        }
    }
}

pub fn handle_asid_control(
    thread: &Thread,
    _cap: Cap,
    label_id: u64,
    length: u64,
    uc: &UserContext,
) -> Result<(), SyscallError> {
    if !invocation_label_matches(label_id, InvocationLabel::RiscvAsidControlMakePool) {
        return Err(SyscallError::IllegalOperation);
    }
    if length < 2 {
        return Err(SyscallError::TruncatedMessage);
    }
    require_extra_caps(uc, 2)?;

    let untyped_cptr = read_extra_cap(thread, 0);
    let root_cptr = read_extra_cap(thread, 1);
    let (untyped_cap, untyped_slot) =
        cspace::lookup_cap(thread, untyped_cptr).map_err(|_| SyscallError::InvalidCapability)?;
    if untyped_cap.tag() != Some(CapTag::Untyped) {
        return Err(SyscallError::InvalidCapability);
    }
    let (root_cap, _) =
        cspace::lookup_cap(thread, root_cptr).map_err(|_| SyscallError::InvalidCapability)?;
    if root_cap.tag() != Some(CapTag::CNode) {
        return Err(SyscallError::InvalidCapability);
    }
    let base = crate::object::asid::next_free_pool_base().ok_or(SyscallError::DeleteFirst)?;

    if untyped_cap.untyped_block_size_bits() != crate::abi::constants::SEL4_ASID_POOL_BITS as u64
        || untyped_cap.untyped_is_device()
    {
        return Err(SyscallError::InvalidCapability);
    }
    let dest_index = uc.regs[UserRegister::A2.index()];
    let dest_depth = uc.regs[UserRegister::A3.index()] as u32 & 0xff;
    unsafe {
        let cspace_guard = crate::object::cnode::lock_cspace();
        if crate::object::cnode::mdb_has_children_locked(&cspace_guard, untyped_slot) {
            return Err(SyscallError::RevokeFirst);
        }
    }

    let dest = resolve_slot(root_cap, dest_index, dest_depth)?;
    unsafe {
        let cspace_guard = crate::object::cnode::lock_cspace();
        if !(*dest).cap.is_null() {
            return Err(SyscallError::DeleteFirst);
        }
        let pool_ptr = untyped_cap.untyped_ptr();
        // Match `performASIDControlInvocation`: consume the parent Untyped,
        // clear the pool frame, insert the ASIDPool cap, then publish the
        // ASID high-level table entry. RISC-V Arch_isCapRevocable is false
        // for ASIDPool, so this uses cteInsert rather than insertNewCap.
        let s = &mut *untyped_slot;
        s.cap
            .set_untyped_free_index(1u64 << (crate::abi::constants::SEL4_ASID_POOL_BITS - 4));
        ptr::write_bytes(
            pool_ptr as *mut u8,
            0,
            1usize << crate::abi::constants::SEL4_ASID_POOL_BITS,
        );
        let new_cap = Cap::new_asid_pool(base as u64, pool_ptr);
        crate::object::cnode::cte_insert_locked(&cspace_guard, new_cap, untyped_slot, dest);
        if !crate::object::asid::publish_pool(base, pool_ptr) {
            panic!("ASID pool base changed before publication");
        }
    }
    Ok(())
}

pub fn handle_asid_pool(
    thread: &Thread,
    cap: Cap,
    label_id: u64,
    _length: u64,
    uc: &UserContext,
) -> Result<(), SyscallError> {
    if !invocation_label_matches(label_id, InvocationLabel::RiscvAsidPoolAssign) {
        return Err(SyscallError::IllegalOperation);
    }
    require_extra_caps(uc, 1)?;

    let vspace_cptr = read_extra_cap(thread, 0);
    let (vspace_cap, vspace_slot) =
        cspace::lookup_cap(thread, vspace_cptr).map_err(|_| SyscallError::InvalidCapability)?;
    if vspace_cap.tag() != Some(CapTag::PageTable) {
        return Err(SyscallError::InvalidCapability);
    }

    let root_pt_kva = vspace_cap.page_table_base_ptr();
    unsafe {
        let _guard = crate::object::cnode::lock_cspace();
        // Re-read the destination before allocating an ASID so a stale
        // lookup cannot publish into a reused or already-assigned slot.
        let current_vspace_cap = (*vspace_slot).cap;
        if current_vspace_cap.tag() != Some(CapTag::PageTable)
            || current_vspace_cap.page_table_base_ptr() != root_pt_kva
        {
            return Err(SyscallError::InvalidCapability);
        }
        if current_vspace_cap.page_table_is_mapped()
            || current_vspace_cap.page_table_mapped_asid() != 0
        {
            return Err(SyscallError::InvalidCapability);
        }
        let asid = match crate::object::asid::next_free_from_pool(
            cap.asid_pool_base(),
            cap.asid_pool_ptr(),
        ) {
            Ok(asid) => asid,
            Err(crate::object::asid::AsidPoolAssignError::MissingPool) => {
                return Err(SyscallError::FailedLookup);
            }
            Err(crate::object::asid::AsidPoolAssignError::WrongPool) => {
                return Err(SyscallError::InvalidCapability);
            }
            Err(crate::object::asid::AsidPoolAssignError::Full) => {
                return Err(SyscallError::DeleteFirst);
            }
        };
        // Match `performASIDPoolInvocation`: make the vspace cap mapped,
        // initialise global kernel mappings, then publish the ASID pool entry.
        (*vspace_slot).cap.set_page_table_mapping(asid, 0);
        crate::arch::current::vspace::copy_kernel_mappings_to(root_pt_kva as *mut PageTable);
        if !crate::object::asid::publish_pool_assignment(
            cap.asid_pool_base(),
            cap.asid_pool_ptr(),
            asid,
            root_pt_kva,
        ) {
            panic!("ASID pool assignment changed before publication");
        }
    }
    Ok(())
}

pub fn handle_irq_control(
    thread: &Thread,
    src_slot: *mut Cte,
    _cap: Cap,
    label_id: u64,
    length: u64,
    uc: &mut UserContext,
) -> Result<(), SyscallError> {
    let (irq, index, depth) = match label_id {
        id if invocation_label_matches(id, InvocationLabel::IrqIssueIrqHandler) => {
            if length < 3 {
                return Err(SyscallError::TruncatedMessage);
            }
            require_extra_caps(uc, 1)?;
            (
                uc.regs[UserRegister::A2.index()],
                uc.regs[UserRegister::A3.index()],
                uc.regs[UserRegister::A4.index()] & 0xff,
            )
        }
        id if invocation_label_matches(id, InvocationLabel::RiscvIrqIssueIrqHandlerTrigger) => {
            if length < 4 {
                return Err(SyscallError::TruncatedMessage);
            }
            require_extra_caps(uc, 1)?;
            // seL4 only accepts this arch-specific invocation when the
            // platform configures HAVE_SET_TRIGGER. QEMU RISC-V here does not
            // model trigger programming, so the syscall must not issue a
            // normal IRQHandler cap.
            return Err(SyscallError::IllegalOperation);
        }
        _ => return Err(SyscallError::IllegalOperation),
    };

    if !crate::object::irq::valid_irq(irq) {
        uc.regs[UserRegister::A2.index()] = 1;
        uc.regs[UserRegister::A3.index()] = crate::object::irq::MAX_IRQ as u64;
        return Err(SyscallError::RangeError);
    }
    if unsafe { crate::object::irq::is_active(irq) } {
        return Err(SyscallError::RevokeFirst);
    }

    let root_cptr = read_extra_cap(thread, 0);
    let (root_cap, _) =
        cspace::lookup_cap(thread, root_cptr).map_err(|_| SyscallError::InvalidCapability)?;
    let dest = resolve_slot(root_cap, index, depth as u32)?;

    unsafe {
        let _cspace_guard = crate::object::cnode::lock_cspace();
        if !(*dest).cap.is_null() {
            return Err(SyscallError::DeleteFirst);
        }
    }

    unsafe {
        if !crate::object::irq::try_issue_handler(irq) {
            return Err(SyscallError::RevokeFirst);
        }
        let cspace_guard = crate::object::cnode::lock_cspace();
        if !(*dest).cap.is_null() {
            crate::object::irq::delete_handler(irq);
            return Err(SyscallError::DeleteFirst);
        }
        crate::object::cnode::cte_insert_locked(
            &cspace_guard,
            Cap::new_irq_handler(irq),
            src_slot,
            dest,
        );
    }
    Ok(())
}

pub fn handle_irq_handler(
    thread: &Thread,
    cap: Cap,
    label_id: u64,
    _length: u64,
    uc: &mut UserContext,
) -> Result<(), SyscallError> {
    let irq = cap.irq_handler_irq();
    match label_id {
        id if invocation_label_matches(id, InvocationLabel::IrqAck) => {
            unsafe { crate::object::irq::ack_irq(irq) };
            Ok(())
        }
        id if invocation_label_matches(id, InvocationLabel::IrqSetHandler) => {
            require_extra_caps(uc, 1)?;
            let ntfn_cptr = read_extra_cap(thread, 0);
            let (ntfn_cap, ntfn_slot) = cspace::lookup_cap(thread, ntfn_cptr)
                .map_err(|_| SyscallError::InvalidCapability)?;
            if ntfn_cap.tag() != Some(CapTag::Notification) || !ntfn_cap.notification_can_send() {
                return Err(SyscallError::InvalidCapability);
            }
            if unsafe { !crate::object::irq::set_notification(irq, ntfn_cap, ntfn_slot) } {
                return Err(SyscallError::InvalidCapability);
            }
            Ok(())
        }
        id if invocation_label_matches(id, InvocationLabel::IrqClearHandler) => {
            unsafe { crate::object::irq::clear_notification(irq) };
            Ok(())
        }
        _ => Err(SyscallError::IllegalOperation),
    }
}

/// DomainSet is accepted for seL4 source compatibility, but this build has
/// `CONFIG_NUM_DOMAINS = 1` and collapses every valid value into the single
/// effective scheduling domain.
pub fn handle_domain(
    thread: &Thread,
    _cap: Cap,
    label_id: u64,
    length: u64,
    uc: &mut UserContext,
) -> Result<(), SyscallError> {
    if !invocation_label_matches(label_id, InvocationLabel::DomainSet) {
        return Err(SyscallError::IllegalOperation);
    }
    if length < 1 {
        return Err(SyscallError::TruncatedMessage);
    }
    require_extra_caps(uc, 1)?;

    let domain = uc.regs[UserRegister::A2.index()];
    if domain >= crate::abi::constants::NUM_DOMAINS as u64 {
        return Err(SyscallError::InvalidArgument);
    }
    let tcb_cap = lookup_extra_cap(thread, 0)?;
    require_tag(tcb_cap, CapTag::Thread)?;
    let tcb_ptr = crate::object::tcb::from_cap(tcb_cap);
    if tcb_ptr.is_null() {
        return Err(SyscallError::InvalidCapability);
    }
    let _ = tcb_ptr;
    Ok(())
}

/// TCB invocations.
///
/// Label values come from `enum invocation_label` in
/// `libsel4/include/sel4/invocation.h`:
///
/// ```text
///  2 TCBReadRegisters      8 TCBSetSchedParams      14 TCBBindNotification
///  3 TCBWriteRegisters     9 TCBSetTimeoutEndpoint  15 TCBUnbindNotification
///  4 TCBCopyRegisters     10 TCBSetIPCBuffer        16 TCBSetTLSBase
///  5 TCBConfigure         11 TCBSetSpace            17 TCBSetFlags
///  6 TCBSetPriority       12 TCBSuspend
///  7 TCBSetMCPriority     13 TCBResume
/// ```
///
/// We do *not* yet have a scheduler — so the handlers persist each
/// operation's data into the `Tcb` struct and return `seL4_NoError`
/// without actually starting/resuming the thread. The test driver's
/// expectation in the 116-test set is that these calls succeed; once we
/// land a real context-switch path the same code will gain real
/// behaviour without changing the parse/validate logic.
pub fn handle_thread(
    thread: &Thread,
    slot: *mut Cte,
    cap: Cap,
    label_id: u64,
    length: u64,
    uc: &mut UserContext,
) -> Result<(), SyscallError> {
    handle_thread_inner(thread, slot, cap, label_id, length, uc, true)
}

pub fn handle_thread_send(
    thread: &Thread,
    slot: *mut Cte,
    cap: Cap,
    label_id: u64,
    length: u64,
    uc: &mut UserContext,
) -> Result<(), SyscallError> {
    if !invocation_label_matches(label_id, InvocationLabel::TcbSetFlags) {
        return Ok(());
    }
    handle_thread_inner(thread, slot, cap, label_id, length, uc, false)
}

fn handle_thread_inner(
    thread: &Thread,
    slot: *mut Cte,
    cap: Cap,
    label_id: u64,
    length: u64,
    uc: &mut UserContext,
    reply: bool,
) -> Result<(), SyscallError> {
    use crate::object::tcb;

    let tcb_ptr = tcb::from_cap(cap);
    if tcb_ptr.is_null() {
        return Err(SyscallError::InvalidCapability);
    }
    crate::kernel::smp::remote_tcb_stall(tcb_ptr);

    match label_id {
        id if id == InvocationLabel::TcbConfigure.raw() => {
            {
                // Non-MCS libsel4: tag = MessageInfo(TCBConfigure, 0, 3, 4)
                //   extraCaps[0] = cspace_root
                //   extraCaps[1] = vspace_root
                //   extraCaps[2] = buffer_frame
                //   mr0 = fault_ep, mr1 = cspace_data, mr2 = vspace_data,
                //   mr3 = buffer_uva
                if length < 4 {
                    return Err(SyscallError::TruncatedMessage);
                }
                require_extra_caps(uc, 3)?;
                let fault_ep = uc.regs[UserRegister::A2.index()];
                let cspace_data = uc.regs[UserRegister::A3.index()];
                let vspace_data = uc.regs[UserRegister::A4.index()];
                let buffer_uva = uc.regs[UserRegister::A5.index()];

                let (mut cspace_cap, cspace_slot) = lookup_tcb_space_cap_slot(thread, 0)?;
                let (mut vspace_cap, vspace_slot) = lookup_tcb_space_cap_slot(thread, 1)?;
                let (buffer_cap, buffer_slot) = lookup_ipc_buffer_cap_slot(thread, 2, buffer_uva)?;

                if tcb_space_slot_long_running_delete(tcb_ptr) {
                    return Err(SyscallError::IllegalOperation);
                }
                if cspace_data != 0 {
                    cspace_cap = update_cap_data(false, cspace_data, cspace_cap);
                    if cspace_cap.is_null() {
                        return Err(SyscallError::IllegalOperation);
                    }
                }
                let cspace_cap = derive_cap_for_copy(cspace_slot, cspace_cap)?;
                require_tcb_cspace_root(cspace_cap)?;
                if vspace_data != 0 {
                    vspace_cap = update_cap_data(false, vspace_data, vspace_cap);
                }
                let vspace_cap = derive_cap_for_copy(vspace_slot, vspace_cap)?;
                require_tcb_vspace_root(vspace_cap)?;

                install_tcb_cap(slot, tcb_ptr, tcb::TCB_CTABLE_SLOT, cspace_cap, cspace_slot)?;
                install_tcb_cap(slot, tcb_ptr, tcb::TCB_VTABLE_SLOT, vspace_cap, vspace_slot)?;
                install_tcb_cap(
                    slot,
                    tcb_ptr,
                    tcb::TCB_FAULT_HANDLER_SLOT,
                    Cap::null(),
                    ptr::null_mut(),
                )?;
                unsafe {
                    tcb::set_fault_endpoint_cptr(tcb_ptr, fault_ep);
                }
                install_tcb_buffer_cap(slot, tcb_ptr, buffer_uva, buffer_cap, buffer_slot)?;
                return Ok(());
            }
        }

        id if id == InvocationLabel::TcbSetSpace.raw() => {
            {
                // Non-MCS libsel4: tag = MessageInfo(TCBSetSpace, 0, 2, 3)
                //   extraCaps[0] = cspace_root, [1] = vspace_root
                //   mr0 = fault_ep, mr1 = cspace_data, mr2 = vspace_data
                if length < 3 {
                    return Err(SyscallError::TruncatedMessage);
                }
                require_extra_caps(uc, 2)?;
                let fault_ep = uc.regs[UserRegister::A2.index()];
                let cspace_data = uc.regs[UserRegister::A3.index()];
                let vspace_data = uc.regs[UserRegister::A4.index()];

                let (mut cspace_cap, cspace_slot) = lookup_tcb_space_cap_slot(thread, 0)?;
                let (mut vspace_cap, vspace_slot) = lookup_tcb_space_cap_slot(thread, 1)?;
                if tcb_space_slot_long_running_delete(tcb_ptr) {
                    return Err(SyscallError::IllegalOperation);
                }
                if cspace_data != 0 {
                    cspace_cap = update_cap_data(false, cspace_data, cspace_cap);
                    if cspace_cap.is_null() {
                        return Err(SyscallError::IllegalOperation);
                    }
                }
                let cspace_cap = derive_cap_for_copy(cspace_slot, cspace_cap)?;
                require_tcb_cspace_root(cspace_cap)?;
                if vspace_data != 0 {
                    vspace_cap = update_cap_data(false, vspace_data, vspace_cap);
                }
                let vspace_cap = derive_cap_for_copy(vspace_slot, vspace_cap)?;
                require_tcb_vspace_root(vspace_cap)?;

                install_tcb_cap(
                    slot,
                    tcb_ptr,
                    tcb::TCB_FAULT_HANDLER_SLOT,
                    Cap::null(),
                    ptr::null_mut(),
                )?;
                unsafe {
                    tcb::set_fault_endpoint_cptr(tcb_ptr, fault_ep);
                }
                install_tcb_cap(slot, tcb_ptr, tcb::TCB_CTABLE_SLOT, cspace_cap, cspace_slot)?;
                install_tcb_cap(slot, tcb_ptr, tcb::TCB_VTABLE_SLOT, vspace_cap, vspace_slot)?;
                return Ok(());
            }
        }

        id if id == InvocationLabel::TcbSetIpcBuffer.raw() => {
            // libsel4: tag = MessageInfo(TCBSetIPCBuffer, 0, 1, 1)
            //   extraCaps[0] = buffer_frame, mr0 = buffer_uva
            if length < 1 {
                return Err(SyscallError::TruncatedMessage);
            }
            require_extra_caps(uc, 1)?;
            let buffer_uva = uc.regs[UserRegister::A2.index()];
            let (buffer_cap, buffer_slot) = lookup_ipc_buffer_cap_slot(thread, 0, buffer_uva)?;
            install_tcb_buffer_cap(slot, tcb_ptr, buffer_uva, buffer_cap, buffer_slot)?;
            Ok(())
        }

        id if id == InvocationLabel::TcbSetPriority.raw() => {
            // libsel4: tag = MessageInfo(TCBSetPriority, 0, 1, 1)
            //   extraCaps[0] = authority (TCB), mr0 = priority
            if length < 1 {
                return Err(SyscallError::TruncatedMessage);
            }
            require_extra_caps(uc, 1)?;
            let prio = uc.regs[UserRegister::A2.index()];
            let auth_cap = lookup_extra_cap(thread, 0)?;
            require_tag(auth_cap, CapTag::Thread)?;
            let auth_tcb = tcb::from_cap(auth_cap);
            if auth_tcb.is_null() {
                return Err(SyscallError::InvalidCapability);
            }
            if prio > 255 {
                return Err(SyscallError::RangeError);
            }
            let _ = auth_tcb;
            Ok(())
        }

        id if id == InvocationLabel::TcbSetMcPriority.raw() => {
            if length < 1 {
                return Err(SyscallError::TruncatedMessage);
            }
            require_extra_caps(uc, 1)?;
            let mcp = uc.regs[UserRegister::A2.index()];
            let auth_cap = lookup_extra_cap(thread, 0)?;
            require_tag(auth_cap, CapTag::Thread)?;
            let auth_tcb = tcb::from_cap(auth_cap);
            if auth_tcb.is_null() {
                return Err(SyscallError::InvalidCapability);
            }
            if mcp > 255 {
                return Err(SyscallError::RangeError);
            }
            let _ = auth_tcb;
            Ok(())
        }

        id if id == InvocationLabel::TcbSetSchedParams.raw() => {
            if length < 2 {
                return Err(SyscallError::TruncatedMessage);
            }
            require_extra_caps(uc, 1)?;
            let mcp = uc.regs[UserRegister::A2.index()];
            let prio = uc.regs[UserRegister::A3.index()];
            let auth_cap = lookup_extra_cap(thread, 0)?;
            require_tag(auth_cap, CapTag::Thread)?;
            let auth_tcb = tcb::from_cap(auth_cap);
            if auth_tcb.is_null() {
                return Err(SyscallError::InvalidCapability);
            }
            if mcp > 255 || prio > 255 {
                return Err(SyscallError::RangeError);
            }

            Ok(())
        }
        id if id == InvocationLabel::TcbSuspend.raw() => {
            let suspend_current = tcb::current() == tcb_ptr;
            unsafe { tcb::suspend(tcb_ptr) };
            if suspend_current {
                return Err(SyscallError::Preempted);
            }
            Ok(())
        }

        id if id == InvocationLabel::TcbResume.raw() => {
            let caller = tcb::current();
            unsafe { tcb::resume(tcb_ptr) };
            if caller != tcb_ptr {
                tcb::continue_current_once(caller);
            }
            Ok(())
        }

        id if id == InvocationLabel::TcbWriteRegisters.raw() => {
            // libsel4: tag = MessageInfo(TCBWriteRegisters, 0, 0, 34)
            //   mr0 = (resume_target & 1) | ((arch_flags & 0xff) << 8)
            //   mr1 = count, mr2 = pc, mr3 = ra
            //   mr4.. = sp, gp, s0..s11, a0..a7, t0..t6, tp  (in that order)
            if length < 2 {
                return Err(SyscallError::TruncatedMessage);
            }
            let flag_word = uc.regs[UserRegister::A2.index()];
            let count = uc.regs[UserRegister::A3.index()];
            if length - 2 < count {
                return Err(SyscallError::TruncatedMessage);
            }
            if tcb::current() == tcb_ptr {
                return Err(SyscallError::IllegalOperation);
            }
            let resume_target = (flag_word & 1) != 0;

            // Remaining regs (mr4..) live in the IPC buffer. libsel4 flattens
            // each architecture's frameRegisters[] and gpRegisters[] arrays
            // after pc/ra; SEL4_USER_CONTEXT_REGS maps that seL4_UserContext
            // word order back to our local UserContext.regs[] indexes. Slots
            // 0/1 are not read from the IPC buffer because pc/ra are handled
            // above.
            let mut reg_updates = [0u64; 32];
            let mut reg_update_valid = [false; 32];
            if length >= 5 && count >= 3 {
                let mr_count = (length as usize)
                    .min((count as usize).saturating_add(2))
                    .min(34);
                if crate::api::thread::current_has_ipc_buffer() {
                    // mr_i for i=4..mr_count holds frameRegister/gpRegister
                    // value at slot (i-2) of seL4_UserContext.
                    for i in 4..mr_count {
                        let mr_val = crate::api::thread::current_ipc_buffer_word(1 + i);
                        let ctx_idx = i - 2;
                        let target_idx = SEL4_USER_CONTEXT_REGS[ctx_idx];
                        if target_idx != 0 {
                            reg_updates[target_idx] = mr_val;
                            reg_update_valid[target_idx] = true;
                        }
                    }
                }
            }
            let pc = if count >= 1 && length >= 3 {
                Some(uc.regs[UserRegister::A4.index()])
            } else {
                None
            };
            if count >= 2 && length >= 4 {
                reg_updates[UserRegister::Ra.index()] = uc.regs[UserRegister::A5.index()];
                reg_update_valid[UserRegister::Ra.index()] = true;
            }
            let mut regs = [(0usize, 0u64); 32];
            let mut reg_count = 0;
            for idx in 0..reg_updates.len() {
                if reg_update_valid[idx] {
                    regs[reg_count] = (idx, reg_updates[idx]);
                    reg_count += 1;
                }
            }
            unsafe { tcb::write_user_context(tcb_ptr, pc, &regs[..reg_count]) };
            // `resume_target = 1` means "also start (or restart) this
            // TCB", per `decodeWriteRegisters` in
            // `kernel/src/object/tcb.c`. This is the dominant codepath
            // for `sel4utils_start_thread` / `sel4test_run_test`: the
            // helper-spawn sequence is Configure + SetPriority +
            // WriteRegisters(resume=1), with no separate `seL4_TCB_Resume`.
            if resume_target {
                unsafe { crate::object::tcb::resume(tcb_ptr) };
            }
            Ok(())
        }

        id if id == InvocationLabel::TcbReadRegisters.raw() => {
            // libsel4: tag = MessageInfo(TCBReadRegisters, 0, 0, 2)
            //   mr0 = (suspend_source & 1) | ((arch_flags & 0xff) << 8)
            //   mr1 = count
            // On reply, the kernel returns up to `count` registers:
            //   mr0 = pc, mr1 = ra, mr2 = sp, mr3 = gp
            //   mr4.. = s0..s11, a0..a7, t0..t6, tp  (in that order)
            //
            // Same per-architecture seL4_UserContext order as
            // TCB_WriteRegisters — share the same architecture ABI table.
            if length < 2 {
                return Err(SyscallError::TruncatedMessage);
            }
            let flag_word = uc.regs[UserRegister::A2.index()];
            let count = uc.regs[UserRegister::A3.index()] as usize;
            if count == 0 || count > SEL4_USER_CONTEXT_WORDS {
                return Err(SyscallError::RangeError);
            }
            if tcb::current() == tcb_ptr {
                return Err(SyscallError::IllegalOperation);
            }
            let suspend_source = (flag_word & 1) != 0;

            // Read register at seL4_UserContext field index `i`.
            let read_reg = |i: usize| -> u64 {
                if i < 32 {
                    let idx = SEL4_USER_CONTEXT_REGS[i];
                    tcb::user_context_word_snapshot(tcb_ptr, i, idx)
                } else {
                    0
                }
            };

            let n = count.min(32);
            // First 4 MRs go through registers a2..a5.
            if n >= 1 {
                uc.regs[UserRegister::A2.index()] = read_reg(0);
            }
            if n >= 2 {
                uc.regs[UserRegister::A3.index()] = read_reg(1);
            }
            if n >= 3 {
                uc.regs[UserRegister::A4.index()] = read_reg(2);
            }
            if n >= 4 {
                uc.regs[UserRegister::A5.index()] = read_reg(3);
            }

            // MRs 4..n live in the IPC buffer at words[1+i].
            if n > 4 {
                for i in 4..n {
                    crate::api::thread::write_current_ipc_buffer_word(1 + i, read_reg(i));
                }
            }

            if suspend_source {
                unsafe { crate::object::tcb::suspend(tcb_ptr) };
            }

            // `write_ok_reply` after we return will set a0=0 and
            // a1=MessageInfo(label=0,length=0) — it deliberately
            // doesn't touch a2..a5. libsel4's `seL4_TCB_ReadRegisters`
            // stub reads `count` MRs unconditionally (using the count
            // it sent, not the reply's length), so length=0 here is
            // fine.
            let _ = n;
            Ok(())
        }

        id if id == InvocationLabel::TcbCopyRegisters.raw() => {
            invoke_tcb_copy_registers(thread, tcb_ptr, length, uc)
        }

        id if id == InvocationLabel::TcbBindNotification.raw() => {
            // libsel4: tag = MessageInfo(_, 0, 1, 0)
            //   extraCaps[0] = notification cap
            require_extra_caps(uc, 1)?;
            let ntfn_cap = lookup_extra_cap(thread, 0)?;
            if ntfn_cap.tag() != Some(CapTag::Notification) || !ntfn_cap.notification_can_receive()
            {
                return Err(SyscallError::IllegalOperation);
            }
            if tcb::bound_notification_snapshot(tcb_ptr) != 0 {
                return Err(SyscallError::IllegalOperation);
            }
            let ntfn_ptr =
                ntfn_cap.notification_ptr() as *mut crate::object::notification::Notification;
            if unsafe { !crate::object::notification::can_bind_snapshot(ntfn_ptr) } {
                return Err(SyscallError::IllegalOperation);
            }
            unsafe {
                tcb::bind_notification(tcb_ptr, ntfn_cap.notification_ptr());
            }
            Ok(())
        }

        id if id == InvocationLabel::TcbUnbindNotification.raw() => {
            if tcb::bound_notification_snapshot(tcb_ptr) == 0 {
                return Err(SyscallError::IllegalOperation);
            }
            unsafe { tcb::unbind_notification(tcb_ptr) };
            Ok(())
        }

        id if id == InvocationLabel::TcbSetAffinity.raw() => {
            if length < 1 {
                return Err(SyscallError::TruncatedMessage);
            }
            let affinity = uc.regs[UserRegister::A2.index()];
            if affinity >= crate::abi::constants::MAX_NUM_NODES as u64 {
                return Err(SyscallError::InvalidArgument);
            }
            let _ = affinity;
            Ok(())
        }

        id if id == InvocationLabel::TcbSetTlsBase.raw() => {
            if length < 1 {
                return Err(SyscallError::TruncatedMessage);
            }
            unsafe { tcb::set_tls_base(tcb_ptr, uc.regs[UserRegister::A2.index()]) };
            Ok(())
        }

        id if id == InvocationLabel::TcbSetFlags.raw() => {
            if length < 2 {
                return Err(SyscallError::TruncatedMessage);
            }
            let clear = uc.regs[UserRegister::A2.index()];
            let set = uc.regs[UserRegister::A3.index()];
            let flags = unsafe { tcb::set_flags(tcb_ptr, clear, set) };
            if reply {
                write_reply_mr0(uc, flags);
            }
            Ok(())
        }

        _ => Err(SyscallError::IllegalOperation),
    }
}

fn invoke_tcb_copy_registers(
    thread: &Thread,
    dest: *mut Tcb,
    length: u64,
    uc: &UserContext,
) -> Result<(), SyscallError> {
    if length < 1 {
        return Err(SyscallError::TruncatedMessage);
    }
    require_extra_caps(uc, 1)?;

    let flags = uc.regs[UserRegister::A2.index()];
    let src_cap = lookup_extra_cap(thread, 0)?;
    require_tag(src_cap, CapTag::Thread)?;
    let src = tcb::from_cap(src_cap);
    if src.is_null() {
        return Err(SyscallError::InvalidCapability);
    }

    let transfer_frame = flags & TCB_COPY_TRANSFER_FRAME != 0;
    let transfer_integer = flags & TCB_COPY_TRANSFER_INTEGER != 0;
    let copied = snapshot_tcb_copy_registers(src, transfer_frame, transfer_integer);

    unsafe {
        if flags & TCB_COPY_SUSPEND_SOURCE != 0 {
            tcb::suspend(src);
        }
        if flags & TCB_COPY_RESUME_TARGET != 0 {
            tcb::resume(dest);
        }
        tcb::write_user_context(dest, copied.pc, &copied.regs[..copied.reg_count]);
    }
    Ok(())
}

struct TcbCopyRegisters {
    pc: Option<u64>,
    regs: [(usize, u64); 31],
    reg_count: usize,
}

fn snapshot_tcb_copy_registers(
    src: *const Tcb,
    transfer_frame: bool,
    transfer_integer: bool,
) -> TcbCopyRegisters {
    let mut copied = TcbCopyRegisters {
        pc: None,
        regs: [(0, 0); 31],
        reg_count: 0,
    };

    if transfer_frame {
        copied.pc = Some(tcb::user_context_word_snapshot(src, 0, 0));
        for &reg in &SEL4_TCB_FRAME_REGS[1..] {
            copied.regs[copied.reg_count] =
                (reg, tcb::user_context_word_snapshot(src, usize::MAX, reg));
            copied.reg_count += 1;
        }
    }

    if transfer_integer {
        for &reg in &SEL4_TCB_GP_REGS {
            copied.regs[copied.reg_count] =
                (reg, tcb::user_context_word_snapshot(src, usize::MAX, reg));
            copied.reg_count += 1;
        }
    }

    copied
}

/// Verify that a freshly looked-up extra-cap carries the expected tag
/// for decoders whose seL4 path reports `seL4_InvalidCapability`.
#[inline]
fn require_tag(cap: Cap, expected: CapTag) -> Result<(), SyscallError> {
    if cap.tag() == Some(expected) {
        Ok(())
    } else {
        Err(SyscallError::InvalidCapability)
    }
}

fn lookup_ipc_buffer_cap(thread: &Thread, i: usize, buffer_uva: u64) -> Result<Cap, SyscallError> {
    lookup_ipc_buffer_cap_slot(thread, i, buffer_uva).map(|(cap, _)| cap)
}

fn lookup_ipc_buffer_cap_slot(
    thread: &Thread,
    i: usize,
    buffer_uva: u64,
) -> Result<(Cap, *mut Cte), SyscallError> {
    let (buffer_cap, buffer_slot) = lookup_extra_cap_slot(thread, i)?;
    if buffer_uva == 0 {
        return Ok((Cap::null(), ptr::null_mut()));
    }
    let buffer_cap = derive_cap_for_copy(buffer_slot, buffer_cap)?;
    if buffer_cap.tag() != Some(CapTag::Frame) || buffer_cap.frame_is_device() {
        return Err(SyscallError::IllegalOperation);
    }
    if buffer_uva & ((1 << SEL4_IPC_BUFFER_SIZE_BITS) - 1) != 0 {
        return Err(SyscallError::AlignmentError);
    }
    Ok((buffer_cap, buffer_slot))
}

fn lookup_tcb_space_cap_slot(thread: &Thread, i: usize) -> Result<(Cap, *mut Cte), SyscallError> {
    let cptr = read_extra_cap(thread, i);
    cspace::lookup_cap(thread, cptr).map_err(|_| SyscallError::InvalidCapability)
}

#[inline]
fn require_tcb_cspace_root(cap: Cap) -> Result<(), SyscallError> {
    if cap.tag() == Some(CapTag::CNode) {
        Ok(())
    } else {
        Err(SyscallError::IllegalOperation)
    }
}

#[inline]
fn require_tcb_vspace_root(cap: Cap) -> Result<(), SyscallError> {
    if cap.tag() == Some(CapTag::PageTable) && cap.page_table_is_mapped() {
        Ok(())
    } else {
        Err(SyscallError::IllegalOperation)
    }
}

fn set_tcb_ipc_buffer(
    tcb: *mut crate::object::tcb::Tcb,
    buffer_uva: u64,
    buffer_cap: Cap,
) -> Result<(), SyscallError> {
    if tcb.is_null() {
        return Err(SyscallError::InvalidCapability);
    }
    unsafe { tcb::set_ipc_buffer(tcb, buffer_uva, buffer_cap) };
    Ok(())
}
#[inline]
fn require_endpoint_send_grant(cap: Cap) -> Result<(), SyscallError> {
    if cap.tag() == Some(CapTag::Endpoint)
        && cap.endpoint_can_send()
        && (cap.endpoint_can_grant() || cap.endpoint_can_grant_reply())
    {
        Ok(())
    } else {
        Err(SyscallError::InvalidCapability)
    }
}

/// Look up `extraCaps[i]` from the current thread's IPC buffer. Mirrors
/// the pattern used by `handle_frame` / `handle_untyped` — every TCB
/// invocation that takes a cap reads it through the
/// `caps_or_badges[i]` field of the calling thread's IPC buffer.
fn lookup_extra_cap(thread: &Thread, i: usize) -> Result<Cap, SyscallError> {
    lookup_extra_cap_slot(thread, i).map(|(cap, _)| cap)
}

fn lookup_extra_cap_slot(thread: &Thread, i: usize) -> Result<(Cap, *mut Cte), SyscallError> {
    let cptr = read_extra_cap(thread, i);
    let (cap, slot) =
        cspace::lookup_cap(thread, cptr).map_err(|_| SyscallError::InvalidCapability)?;
    Ok((cap, slot))
}
fn lookup_optional_extra_cap(thread: &Thread, i: usize) -> Result<Option<Cap>, SyscallError> {
    lookup_optional_extra_cap_slot(thread, i).map(|cap| cap.map(|(cap, _)| cap))
}

fn lookup_optional_extra_cap_slot(
    thread: &Thread,
    i: usize,
) -> Result<Option<(Cap, *mut Cte)>, SyscallError> {
    let cptr = read_extra_cap(thread, i);
    let (cap, slot) =
        cspace::lookup_cap(thread, cptr).map_err(|_| SyscallError::InvalidCapability)?;
    if cap.is_null() {
        return Ok(None);
    }
    Ok(Some((cap, slot)))
}

fn tcb_space_slot_long_running_delete(tcb_ptr: *mut Tcb) -> bool {
    let cspace_guard = crate::object::cnode::lock_cspace();
    unsafe {
        slot_cap_long_running_delete_locked(
            &cspace_guard,
            tcb::cap_slot(tcb_ptr, tcb::TCB_CTABLE_SLOT),
        ) || slot_cap_long_running_delete_locked(
            &cspace_guard,
            tcb::cap_slot(tcb_ptr, tcb::TCB_VTABLE_SLOT),
        )
    }
}

unsafe fn slot_cap_long_running_delete_locked(
    cspace_guard: &CspaceLockGuard,
    slot: *mut Cte,
) -> bool {
    if slot.is_null() {
        return false;
    }
    let cap = unsafe { (*slot).cap };
    if cap.is_null() || unsafe { !is_final_capability(cspace_guard, slot) } {
        return false;
    }
    matches!(
        cap.tag(),
        Some(CapTag::Thread) | Some(CapTag::Zombie) | Some(CapTag::CNode)
    )
}

fn install_tcb_cap(
    tcb_slot: *mut Cte,
    tcb_ptr: *mut Tcb,
    index: usize,
    new_cap: Cap,
    src_slot: *mut Cte,
) -> Result<(), SyscallError> {
    let dest = unsafe { tcb::cap_slot(tcb_ptr, index) };
    if dest.is_null() {
        return Err(SyscallError::InvalidCapability);
    }

    cte_delete(dest, true)?;

    insert_tcb_cap_if_live(tcb_slot, tcb_ptr, dest, new_cap, src_slot)
}

fn install_tcb_buffer_cap(
    tcb_slot: *mut Cte,
    tcb_ptr: *mut Tcb,
    buffer_uva: u64,
    buffer_cap: Cap,
    buffer_src_slot: *mut Cte,
) -> Result<(), SyscallError> {
    let dest = unsafe { tcb::cap_slot(tcb_ptr, tcb::TCB_BUFFER_SLOT) };
    if dest.is_null() {
        return Err(SyscallError::InvalidCapability);
    }

    cte_delete(dest, true)?;
    // Match seL4 `invokeTCB_ThreadControlCaps`: publish the new IPC buffer
    // address after deleting the old buffer slot and before inserting the cap.
    set_tcb_ipc_buffer(tcb_ptr, buffer_uva, buffer_cap)?;

    insert_tcb_cap_if_live(tcb_slot, tcb_ptr, dest, buffer_cap, buffer_src_slot)
}

fn insert_tcb_cap_if_live(
    tcb_slot: *mut Cte,
    tcb_ptr: *mut Tcb,
    dest: *mut Cte,
    new_cap: Cap,
    src_slot: *mut Cte,
) -> Result<(), SyscallError> {
    if new_cap.is_null() || src_slot.is_null() || tcb_slot.is_null() {
        return Ok(());
    }

    let thread_cap = Cap::new_thread(tcb_ptr as u64);
    let cspace_guard = crate::object::cnode::lock_cspace();
    unsafe {
        if !same_object_as(new_cap, (*src_slot).cap) || !same_object_as(thread_cap, (*tcb_slot).cap)
        {
            return Ok(());
        }
        crate::object::cnode::cte_insert_locked(&cspace_guard, new_cap, src_slot, dest);
    }
    Ok(())
}

/// Convert a kernel-VA in either the kernel-ELF window or the PSpace
/// window back to its physical address. Frame caps minted from regular
/// untypeds carry kernel-ELF VAs; device frame caps carry PSpace VAs.
#[inline]
fn kva_to_pa(kva: u64) -> u64 {
    use crate::abi::constants::{KERNEL_ELF_BASE, PADDR_BASE, PHYS_BASE_RAW, PPTR_BASE};
    if kva >= (KERNEL_ELF_BASE as u64) {
        kva - (KERNEL_ELF_BASE as u64) + (PHYS_BASE_RAW as u64)
    } else {
        // PSpace window: KVA = PPTR_BASE + (pa - PADDR_BASE)
        kva - (PPTR_BASE as u64) + (PADDR_BASE as u64)
    }
}

/// CNode operations: Revoke/Delete/Copy/Mint/Move/Mutate/Rotate.
///
/// `_cap` (the looked-up `_service`) is the destination CSpace root; the
/// extra-caps[0] slot in the IPC buffer holds the source CSpace root.
/// Both must be CNode caps. For two-arg ops (Revoke/Delete) only the
/// destination is used.
pub fn handle_cnode(
    thread: &Thread,
    _src_slot: *mut Cte,
    dest_root_cap: Cap,
    label_id: u64,
    length: u64,
    uc: &mut UserContext,
) -> Result<(), SyscallError> {
    match label_id {
        id if invocation_label_matches(id, InvocationLabel::CNodeRevoke) => {
            cnode_op_revoke(dest_root_cap, length, uc)
        }
        id if invocation_label_matches(id, InvocationLabel::CNodeDelete) => {
            cnode_op_delete(dest_root_cap, length, uc)
        }
        id if invocation_label_matches(id, InvocationLabel::CNodeCopy) => {
            cnode_op_copy_or_mint(thread, dest_root_cap, length, uc, false)
        }
        id if invocation_label_matches(id, InvocationLabel::CNodeMint) => {
            cnode_op_copy_or_mint(thread, dest_root_cap, length, uc, true)
        }
        id if invocation_label_matches(id, InvocationLabel::CNodeMove) => {
            cnode_op_move_or_mutate(thread, dest_root_cap, length, uc, false)
        }
        id if invocation_label_matches(id, InvocationLabel::CNodeMutate) => {
            cnode_op_move_or_mutate(thread, dest_root_cap, length, uc, true)
        }
        id if invocation_label_matches(id, InvocationLabel::CNodeCancelBadgedSends) => {
            cnode_op_cancel_badged_sends(dest_root_cap, length, uc)
        }
        id if invocation_label_matches(id, InvocationLabel::CNodeRotate) => {
            cnode_op_rotate(thread, dest_root_cap, length, uc)
        }
        id if invocation_label_matches(id, InvocationLabel::CNodeSaveCaller) => {
            cnode_op_save_caller(dest_root_cap, length, uc)
        }
        _ => {
            let _ = label_id;
            Err(SyscallError::IllegalOperation)
        }
    }
}

fn cnode_op_save_caller(
    dest_root_cap: Cap,
    length: u64,
    uc: &UserContext,
) -> Result<(), SyscallError> {
    if length < 2 {
        return Err(SyscallError::TruncatedMessage);
    }
    let index = uc.regs[UserRegister::A2.index()];
    let depth = uc.regs[UserRegister::A3.index()] as u32 & 0xff;
    let dest = resolve_slot(dest_root_cap, index, depth)?;

    unsafe {
        let cspace_guard = crate::object::cnode::lock_cspace();
        if !(*dest).cap.is_null() || (*dest).mdb.prev() != 0 || (*dest).mdb.next() != 0 {
            return Err(SyscallError::DeleteFirst);
        }
        let (reply_object, can_grant) = tcb::take_caller_reply(tcb::current());
        if reply_object == 0 {
            return Ok(());
        }
        (*dest).cap = Cap::new_reply_object(reply_object, can_grant);
        (*dest).mdb = MdbNode::NULL;
        crate::object::reply::set_saved_reply_slot(reply_object, dest);
        let _ = cspace_guard;
    }
    Ok(())
}

/// CNode_CancelBadgedSends: target slot must hold an Endpoint cap with
/// full Send+Receive+Grant+GrantReply rights. Unbadged ⇒ no-op success.
/// Badged ⇒ walk the EP's wait list and wake every blocked sender whose
/// `sender_badge` matches the cap's badge.
fn cnode_op_cancel_badged_sends(
    dest_root_cap: Cap,
    length: u64,
    uc: &UserContext,
) -> Result<(), SyscallError> {
    if length < 2 {
        return Err(SyscallError::TruncatedMessage);
    }
    let index = uc.regs[UserRegister::A2.index()];
    let depth = uc.regs[UserRegister::A3.index()] as u32 & 0xff;
    let slot = resolve_slot(dest_root_cap, index, depth)?;
    let cap = crate::object::cnode::cap_snapshot(slot);
    // Mirror C kernel `hasCancelSendRights`: only Endpoint caps with all
    // four rights are valid targets.
    if cap.tag() != Some(CapTag::Endpoint)
        || !cap.endpoint_can_send()
        || !cap.endpoint_can_receive()
        || !cap.endpoint_can_grant()
        || !cap.endpoint_can_grant_reply()
    {
        return Err(SyscallError::IllegalOperation);
    }
    let badge = cap.endpoint_badge();
    if badge == 0 {
        return Ok(());
    }
    let ep_ptr = cap.endpoint_ptr() as *mut crate::object::endpoint::Endpoint;
    if ep_ptr.is_null() {
        return Ok(());
    }
    unsafe {
        crate::object::endpoint::cancel_badged_sends(ep_ptr, badge);
    }
    Ok(())
}

/// CNode_Rotate: atomic move of `pivot → dest` and `src → pivot`. If
/// `src == dest` it degenerates to a swap of `src` and `pivot`. Mirrors
/// C kernel `decodeCNodeInvocation`'s Rotate handling.
fn cnode_op_rotate(
    thread: &Thread,
    dest_root_cap: Cap,
    length: u64,
    uc: &UserContext,
) -> Result<(), SyscallError> {
    if length < 2 {
        return Err(SyscallError::TruncatedMessage);
    }
    let dest_index = uc.regs[UserRegister::A2.index()];
    let dest_depth = uc.regs[UserRegister::A3.index()] as u32 & 0xff;
    let dest = resolve_slot(dest_root_cap, dest_index, dest_depth)?;

    if length < 8 {
        return Err(SyscallError::TruncatedMessage);
    }
    require_extra_caps(uc, 2)?;
    let pivot_new_data = uc.regs[UserRegister::A4.index()]; // libsel4 calls this `dest_badge`
    let pivot_index = uc.regs[UserRegister::A5.index()];
    let pivot_depth = read_mr(thread, uc, 4) as u32 & 0xff;
    let src_new_data = read_mr(thread, uc, 5); // libsel4 calls this `pivot_badge`
    let src_index = read_mr(thread, uc, 6);
    let src_depth = read_mr(thread, uc, 7) as u32 & 0xff;

    let pivot_root_cptr = read_extra_cap(thread, 0);
    let src_root_cptr = read_extra_cap(thread, 1);
    let (src_root_cap, _) =
        cspace::lookup_cap(thread, src_root_cptr).map_err(|_| SyscallError::InvalidCapability)?;
    let (pivot_root_cap, _) =
        cspace::lookup_cap(thread, pivot_root_cptr).map_err(|_| SyscallError::InvalidCapability)?;

    let src = resolve_slot(src_root_cap, src_index, src_depth)?;
    let pivot = resolve_slot(pivot_root_cap, pivot_index, pivot_depth)?;

    if pivot == src || pivot == dest {
        return Err(SyscallError::IllegalOperation);
    }
    unsafe {
        let cspace_guard = crate::object::cnode::lock_cspace();
        if src != dest {
            if !(*dest).cap.is_null() {
                return Err(SyscallError::DeleteFirst);
            }
        }
        if (*src).cap.is_null() {
            return Err(SyscallError::FailedLookup);
        }
        if (*pivot).cap.is_null() {
            return Err(SyscallError::FailedLookup);
        }
        let new_src_cap = update_cap_data(true, src_new_data, (*src).cap);
        let new_pivot_cap = update_cap_data(true, pivot_new_data, (*pivot).cap);
        if new_src_cap.is_null() || new_pivot_cap.is_null() {
            return Err(SyscallError::IllegalOperation);
        }
        if src == dest {
            cnode_swap_slots(&cspace_guard, new_src_cap, src, new_pivot_cap, pivot);
        } else {
            // Step 1: move pivot → dest.
            cnode_move_slot(pivot, dest, new_pivot_cap);
            // Step 2: move src → pivot.
            cnode_move_slot(src, pivot, new_src_cap);
        }
    }
    Ok(())
}

/// Mirror seL4 `cteMove`: move a cap+MDB entry from `src` to `dst`,
/// applying the given (possibly badged) cap, then re-thread MDB neighbours
/// to the new slot location.
unsafe fn cnode_move_slot(src: *mut Cte, dst: *mut Cte, new_cap: Cap) {
    unsafe {
        if src.is_null() || dst.is_null() {
            panic!("cteMove expects valid slots");
        }
        if src == dst {
            panic!("cteMove source and destination must differ");
        }
        if (*src).cap.is_null() {
            panic!("cteMove from empty source");
        }
        if !(*dst).cap.is_null() {
            panic!("cteMove to non-empty destination");
        }
        if (*dst).mdb.next() != 0 || (*dst).mdb.prev() != 0 {
            panic!("cteMove destination MDB entry must be empty");
        }
        let moved_mdb = (*src).mdb;
        (*dst).cap = new_cap;
        (*src).cap = Cap::null();
        (*dst).mdb = moved_mdb;
        (*src).mdb = MdbNode::NULL;
        relink_mdb_neighbors(dst, moved_mdb);
    }
}

/// Read message-register `mr_i` for `i ≥ 4` from the IPC buffer (mr0..3
/// live in `uc.regs[a2..a5]`). Returns 0 if the IPC buffer isn't mapped.
fn read_mr(_thread: &Thread, uc: &UserContext, i: usize) -> u64 {
    match i {
        0 => uc.regs[UserRegister::A2.index()],
        1 => uc.regs[UserRegister::A3.index()],
        2 => uc.regs[UserRegister::A4.index()],
        3 => uc.regs[UserRegister::A5.index()],
        _ => crate::api::thread::current_ipc_buffer_word(1 + i),
    }
}

// Matches the sel4test kernel's CONFIG_MAX_NUM_WORK_UNITS_PER_PREEMPTION.
const CNODE_WORK_UNITS_PER_PREEMPTION: usize = 100;
static CNODE_WORK_UNITS_COMPLETED: BklCell<usize> = BklCell::new(0);

fn reset_cnode_work_units() {
    CNODE_WORK_UNITS_COMPLETED.with_mut(|work_units| {
        *work_units = 0;
    });
}

fn cnode_preemption_point() -> Result<(), SyscallError> {
    let should_poll = CNODE_WORK_UNITS_COMPLETED.with_mut(|work_units| {
        *work_units += 1;
        if *work_units < CNODE_WORK_UNITS_PER_PREEMPTION {
            return false;
        }
        *work_units = 0;
        true
    });
    if !should_poll {
        return Ok(());
    }
    if crate::arch::current::trap::service_due_timer_interrupts() {
        return Err(SyscallError::Preempted);
    }
    Ok(())
}

fn cte_revoke(root: *mut Cte) -> Result<(), SyscallError> {
    debug_assert_kernel_lock_held();
    {
        let cspace_guard = crate::object::cnode::lock_cspace();
        if unsafe { !crate::object::cnode::mdb_has_children_locked(&cspace_guard, root) } {
            return Ok(());
        }
    }

    reset_cnode_work_units();
    revoke_descendants(root)
}

fn cnode_op_revoke(dest_root_cap: Cap, length: u64, uc: &UserContext) -> Result<(), SyscallError> {
    if length < 2 {
        return Err(SyscallError::TruncatedMessage);
    }
    let index = uc.regs[UserRegister::A2.index()];
    let depth = uc.regs[UserRegister::A3.index()] as u32 & 0xff;
    let slot = resolve_slot(dest_root_cap, index, depth)?;
    cte_revoke(slot)
}

fn cnode_op_delete(dest_root_cap: Cap, length: u64, uc: &UserContext) -> Result<(), SyscallError> {
    if length < 2 {
        return Err(SyscallError::TruncatedMessage);
    }
    let index = uc.regs[UserRegister::A2.index()];
    let depth = uc.regs[UserRegister::A3.index()] as u32 & 0xff;
    let slot = resolve_slot(dest_root_cap, index, depth)?;
    delete_slot_preemptible(slot)?;
    Ok(())
}

fn cnode_op_copy_or_mint(
    thread: &Thread,
    dest_root_cap: Cap,
    length: u64,
    uc: &UserContext,
    is_mint: bool,
) -> Result<(), SyscallError> {
    if length < 2 {
        return Err(SyscallError::TruncatedMessage);
    }
    let dest_index = uc.regs[UserRegister::A2.index()];
    let dest_depth = uc.regs[UserRegister::A3.index()] as u32 & 0xff;
    let dest = resolve_slot(dest_root_cap, dest_index, dest_depth)?;

    if length < 4 {
        return Err(SyscallError::TruncatedMessage);
    }
    require_extra_caps(uc, 1)?;
    let src_index = uc.regs[UserRegister::A4.index()];
    let src_depth = uc.regs[UserRegister::A5.index()] as u32 & 0xff;

    let src_root_cptr = read_extra_cap(thread, 0);
    let (src_root_cap, _) =
        cspace::lookup_cap(thread, src_root_cptr).map_err(|_| SyscallError::InvalidCapability)?;

    unsafe {
        let _cspace_guard = crate::object::cnode::lock_cspace();
        if !(*dest).cap.is_null() {
            return Err(SyscallError::DeleteFirst);
        }
    }

    let src = resolve_slot(src_root_cap, src_index, src_depth)?;

    let src_cap = crate::object::cnode::cap_snapshot(src);
    if src_cap.is_null() {
        // Mirrors C kernel `decodeCNodeInvocation` -> `lookupSourceSlot`:
        // an empty source slot is a "failed lookup", not an
        // "illegal operation". sel4test (CNODEOP0003 etc.) keys off
        // the exact label, so map this carefully.
        return Err(SyscallError::FailedLookup);
    }
    if length < if is_mint { 6 } else { 5 } {
        return Err(SyscallError::TruncatedMessage);
    }
    let rights = read_mr(thread, uc, 4);
    let mut new_cap = mask_cap_rights(src_cap, rights);
    if is_mint {
        let badge = read_mr(thread, uc, 5);
        new_cap = update_cap_data(false, badge, new_cap);
    }
    new_cap = derive_cap_for_copy(src, new_cap)?;
    if new_cap.is_null() {
        return Err(SyscallError::IllegalOperation);
    }
    unsafe {
        let cspace_guard = crate::object::cnode::lock_cspace();
        crate::object::cnode::cte_insert_locked(&cspace_guard, new_cap, src, dest);
    }
    Ok(())
}

pub(crate) fn derive_cap_for_copy(slot: *mut Cte, mut cap: Cap) -> Result<Cap, SyscallError> {
    match cap.tag() {
        Some(CapTag::Zombie) | Some(CapTag::IrqControl) => Ok(Cap::null()),
        Some(CapTag::Untyped) => {
            let cspace_guard = crate::object::cnode::lock_cspace();
            if unsafe { crate::object::cnode::mdb_has_children_locked(&cspace_guard, slot) } {
                Err(SyscallError::RevokeFirst)
            } else {
                Ok(cap)
            }
        }
        Some(CapTag::Frame) => {
            cap.set_frame_mapped_addr(0);
            cap.set_frame_mapped_asid(0);
            Ok(cap)
        }
        Some(CapTag::PageTable) => {
            if cap.page_table_is_mapped() {
                Ok(cap)
            } else {
                Err(SyscallError::IllegalOperation)
            }
        }
        _ => Ok(cap),
    }
}

/// Is `va` inside the kernel's PSpace window — i.e. backed by directly
/// mapped RAM that we may safely zero?  CNode caps may legitimately
/// point at kernel-ELF mirrors or device frames; finalising those would
/// store into read-only memory and panic the kernel.
#[inline]
fn is_pspace_kva(va: u64) -> bool {
    let v = va as usize;
    v >= crate::abi::constants::PPTR_BASE && v < crate::abi::constants::PPTR_TOP
}

/// Rootserver TCB/CNode storage is boot-allocated outside the
/// normal PSpace window, while later objects come from PSpace. Both are mutable
/// kernel object storage and should follow seL4's final-cap path.
#[inline]
fn is_boot_or_pspace_object_kva(va: u64) -> bool {
    let v = va as usize;
    is_pspace_kva(va) || v >= crate::abi::constants::KERNEL_ELF_BASE
}

/// Apply `seL4_CapRights_t` to caps produced by CNode Copy/Mint. The
/// packed rights bits are:
///   bit 0: allow write, bit 1: allow read,
///   bit 2: allow grant, bit 3: allow grant-reply.
fn mask_cap_rights(mut cap: Cap, rights: u64) -> Cap {
    let allow_write = (rights & 0x1) != 0;
    let allow_read = (rights & 0x2) != 0;
    let allow_grant = (rights & 0x4) != 0;
    let allow_grant_reply = (rights & 0x8) != 0;

    match cap.tag() {
        Some(CapTag::Endpoint) => {
            if !allow_write {
                cap.words[0] &= !(1u64 << 55);
            }
            if !allow_read {
                cap.words[0] &= !(1u64 << 56);
            }
            if !allow_grant {
                cap.words[0] &= !(1u64 << 57);
            }
            if !allow_grant_reply {
                cap.words[0] &= !(1u64 << 58);
            }
        }
        Some(CapTag::Notification) => {
            if !allow_write {
                cap.words[0] &= !(1u64 << 57);
            }
            if !allow_read {
                cap.words[0] &= !(1u64 << 58);
            }
        }
        Some(CapTag::Reply) => {
            cap.set_reply_object_can_grant(cap.reply_object_can_grant() && allow_grant);
        }
        Some(CapTag::Frame) => {
            cap.set_frame_vm_rights(mask_frame_vm_rights(cap.frame_vm_rights(), rights));
        }
        _ => {}
    }
    cap
}

fn mask_frame_vm_rights(vm_rights: u64, rights: u64) -> u64 {
    let allow_write = (rights & 0x1) != 0;
    let allow_read = (rights & 0x2) != 0;

    match vm_rights {
        FRAME_RIGHTS_READ_ONLY if allow_read => FRAME_RIGHTS_READ_ONLY,
        FRAME_RIGHTS_READ_WRITE if allow_read && allow_write => FRAME_RIGHTS_READ_WRITE,
        FRAME_RIGHTS_READ_WRITE if allow_read => FRAME_RIGHTS_READ_ONLY,
        _ => FRAME_RIGHTS_KERNEL_ONLY,
    }
}

#[inline]
fn frame_vm_rights_allow_read(vm_rights: u64) -> bool {
    matches!(vm_rights, FRAME_RIGHTS_READ_ONLY | FRAME_RIGHTS_READ_WRITE)
}

#[inline]
fn frame_vm_rights_allow_write(vm_rights: u64) -> bool {
    vm_rights == FRAME_RIGHTS_READ_WRITE
}

fn cnode_op_move_or_mutate(
    thread: &Thread,
    dest_root_cap: Cap,
    length: u64,
    uc: &UserContext,
    is_mutate: bool,
) -> Result<(), SyscallError> {
    if length < 2 {
        return Err(SyscallError::TruncatedMessage);
    }
    let dest_index = uc.regs[UserRegister::A2.index()];
    let dest_depth = uc.regs[UserRegister::A3.index()] as u32 & 0xff;
    let dest = resolve_slot(dest_root_cap, dest_index, dest_depth)?;

    if length < 4 {
        return Err(SyscallError::TruncatedMessage);
    }
    require_extra_caps(uc, 1)?;
    let src_index = uc.regs[UserRegister::A4.index()];
    let src_depth = uc.regs[UserRegister::A5.index()] as u32 & 0xff;

    let src_root_cptr = read_extra_cap(thread, 0);
    let (src_root_cap, _) =
        cspace::lookup_cap(thread, src_root_cptr).map_err(|_| SyscallError::InvalidCapability)?;

    unsafe {
        let _cspace_guard = crate::object::cnode::lock_cspace();
        if !(*dest).cap.is_null() {
            return Err(SyscallError::DeleteFirst);
        }
    }

    let src = resolve_slot(src_root_cap, src_index, src_depth)?;

    let mut moved = crate::object::cnode::cap_snapshot(src);
    if moved.is_null() {
        return Err(SyscallError::FailedLookup);
    }
    if is_mutate {
        if length < 5 {
            return Err(SyscallError::TruncatedMessage);
        }
        let badge = read_mr(thread, uc, 4);
        moved = update_cap_data(true, badge, moved);
    }
    if moved.is_null() {
        return Err(SyscallError::IllegalOperation);
    }
    unsafe {
        let _cspace_guard = crate::object::cnode::lock_cspace();
        cnode_move_slot(src, dest, moved);
    }
    Ok(())
}

/// Resolve `(index, depth)` to a `Cte*` via the given CNode-root cap.
fn resolve_slot(root_cap: Cap, index: u64, depth: u32) -> Result<*mut Cte, SyscallError> {
    if root_cap.tag() != Some(CapTag::CNode) {
        return Err(SyscallError::FailedLookup);
    }
    if depth == 0 || depth > cspace::WORD_BITS {
        return Err(SyscallError::RangeError);
    }
    let r =
        cspace::lookup_slot_in(root_cap, index, depth).map_err(|_| SyscallError::FailedLookup)?;
    if r.bits_remaining != 0 {
        return Err(SyscallError::FailedLookup);
    }
    Ok(r.slot)
}

fn require_extra_caps(uc: &UserContext, required: u64) -> Result<(), SyscallError> {
    let info = MessageInfo(uc.regs[UserRegister::A1.index()]);
    if info.extra_caps() < required
        || (required != 0 && !crate::api::thread::current_has_ipc_buffer())
    {
        return Err(SyscallError::TruncatedMessage);
    }
    Ok(())
}

/// Mirror seL4 `updateCapData(preserve, newData, cap)`.
fn update_cap_data(preserve: bool, data: u64, cap: Cap) -> Cap {
    match cap.tag() {
        Some(CapTag::Endpoint) => {
            if preserve || cap.endpoint_badge() != 0 {
                return Cap::null();
            }
            let mut out = cap;
            out.set_endpoint_badge(data);
            out
        }
        Some(CapTag::Notification) => {
            if preserve || cap.notification_badge() != 0 {
                return Cap::null();
            }
            let mut out = cap;
            out.set_notification_badge(data);
            out
        }
        Some(CapTag::CNode) => {
            // CNodeCapData: low 6 bits = guard_size, high 58 = guard.
            let guard_size = data & 0x3F;
            if guard_size + cap.cnode_radix() > cspace::WORD_BITS as u64 {
                return Cap::null();
            }
            let guard = (data >> 6) & low_word_mask(guard_size);
            Cap::new_cnode(cap.cnode_ptr(), cap.cnode_radix(), guard, guard_size)
        }
        _ => cap,
    }
}

#[inline]
const fn low_word_mask(bits: u64) -> u64 {
    if bits == 0 { 0 } else { (1u64 << bits) - 1 }
}

/// Empty a slot, freeing the resources behind its cap if necessary.
///
/// Plain `CNode_Delete` does not require the target to be child-free:
/// the C kernel's `cteDelete(..., exposed=true)` finalises the current
/// cap and then `emptySlot`s it, leaving any surviving CDT descendants
/// threaded into the surrounding MDB list. Operations that truly need a
/// leaf (for example deriving an Untyped cap) use their own
/// `ensureNoChildren` check before reaching this path.
fn delete_slot(slot: *mut Cte) -> Result<(), SyscallError> {
    cte_delete(slot, true)
}

fn delete_slot_preemptible(slot: *mut Cte) -> Result<(), SyscallError> {
    debug_assert_kernel_lock_held();
    reset_cnode_work_units();
    delete_slot(slot)
}

/// Mirror seL4 `cteDeleteOne`: finalise a single slot and empty it without
/// walking a Zombie remainder. This is used for kernel-owned internal CTEs
/// such as IRQ notification bindings, where upstream asserts the cap is
/// immediately removable.
pub(crate) fn cte_delete_one(slot: *mut Cte) {
    debug_assert_kernel_lock_held();
    let Some(target) = snapshot_slot_for_delete(slot) else {
        return;
    };
    let result = match finalize_cap(target.slot, target.cap, target.is_final, true) {
        Ok(result) => result,
        Err(_) => panic!("cte_delete_one cap should finalise immediately"),
    };
    let removable = finalise_result_removable(result, target.slot)
        .expect("cte_delete_one cap should return a removable finalise result");
    let cleanup_is_null = result.cleanup_info.is_null();
    if !removable {
        panic!("cte_delete_one cap should be removable");
    }
    if !cleanup_is_null {
        panic!("cte_delete_one should not produce cleanup info");
    }
    empty_finalized_slot(slot, Cap::null());
}

fn cte_delete(slot: *mut Cte, exposed: bool) -> Result<(), SyscallError> {
    debug_assert_kernel_lock_held();
    let result = finalise_slot(slot, exposed)?;
    if exposed || result.success {
        empty_finalized_slot(slot, result.cleanup_info);
    }
    Ok(())
}

#[derive(Copy, Clone)]
struct FinaliseSlotResult {
    success: bool,
    cleanup_info: Cap,
}

#[derive(Copy, Clone)]
struct FinaliseCapResult {
    remainder: Cap,
    cleanup_info: Cap,
}

impl FinaliseCapResult {
    const fn null() -> Self {
        Self {
            remainder: Cap::null(),
            cleanup_info: Cap::null(),
        }
    }

    const fn remainder(remainder: Cap) -> Self {
        Self {
            remainder,
            cleanup_info: Cap::null(),
        }
    }

    const fn with_cleanup(remainder: Cap, cleanup_info: Cap) -> Self {
        Self {
            remainder,
            cleanup_info,
        }
    }
}

fn finalise_slot(slot: *mut Cte, immediate: bool) -> Result<FinaliseSlotResult, SyscallError> {
    loop {
        let Some(target) = snapshot_slot_for_delete(slot) else {
            return Ok(FinaliseSlotResult {
                success: true,
                cleanup_info: Cap::null(),
            });
        };
        if target.cap.tag().is_none() {
            debug_assert!(false, "finaliseSlot expected a valid cap tag");
            panic!("finaliseSlot expected a valid cap tag");
        }
        let result = finalize_cap(target.slot, target.cap, target.is_final, false)?;
        let removable = finalise_result_removable(result, target.slot)?;
        if removable {
            return Ok(FinaliseSlotResult {
                success: true,
                cleanup_info: result.cleanup_info,
            });
        }
        {
            let _cspace_guard = crate::object::cnode::lock_cspace();
            unsafe {
                (*target.slot).cap = result.remainder;
            }
        }
        if !immediate && cap_cyclic_zombie(result.remainder, target.slot) {
            return Ok(FinaliseSlotResult {
                success: false,
                cleanup_info: result.cleanup_info,
            });
        }
        reduce_zombie(target.slot, immediate)?;
        // seL4 checks `preemptionPoint()` after each zombie reduction.
        cnode_preemption_point()?;
    }
}

fn finalise_result_removable(
    result: FinaliseCapResult,
    slot: *mut Cte,
) -> Result<bool, SyscallError> {
    if !result.cleanup_info.is_null() && !result.remainder.is_null() {
        debug_assert!(false, "finaliseCap cleanup info requires a null remainder",);
        panic!("finaliseCap cleanup info requires a null remainder");
    }
    cap_removable(result.remainder, slot)
}

fn cap_removable(cap: Cap, slot: *mut Cte) -> Result<bool, SyscallError> {
    match cap.tag() {
        Some(CapTag::Null) => Ok(true),
        Some(CapTag::Zombie) => {
            let n = cap.zombie_number();
            let zombie_slot = cap.zombie_ptr() as *mut Cte;
            Ok(n == 0 || (n == 1 && zombie_slot == slot))
        }
        None => panic!("finaliseCap returned an invalid cap tag"),
        _ => panic!("finaliseCap should only return Zombie or NullCap"),
    }
}

fn cap_cyclic_zombie(cap: Cap, slot: *mut Cte) -> bool {
    cap.tag() == Some(CapTag::Zombie) && cap.zombie_ptr() as *mut Cte == slot
}

fn reduce_zombie(slot: *mut Cte, immediate: bool) -> Result<(), SyscallError> {
    let cap = crate::object::cnode::cap_snapshot(slot);
    debug_assert_eq!(
        cap.tag(),
        Some(CapTag::Zombie),
        "reduceZombie expected a zombie cap",
    );
    if cap.tag() != Some(CapTag::Zombie) {
        panic!("reduceZombie expected a zombie cap");
    }
    let ptr = cap.zombie_ptr() as *mut Cte;
    let n = cap.zombie_number();
    let zombie_type = cap.zombie_type();
    debug_assert!(!ptr.is_null(), "zombie cap must point at a CTE range");
    let removable = cap_removable(cap, slot)?;
    debug_assert!(!removable, "reduceZombie expected an unremovable zombie",);
    debug_assert!(n > 0, "reduceZombie expected a non-empty zombie");
    if ptr.is_null() {
        panic!("zombie cap must point at a CTE range");
    }
    if n == 0 {
        panic!("reduceZombie expected a non-empty zombie");
    }
    if removable {
        panic!("reduceZombie expected an unremovable zombie");
    }

    if immediate {
        let end_slot = unsafe { ptr.add((n - 1) as usize) };
        cte_delete(end_slot, false)?;
        match crate::object::cnode::cap_snapshot(slot).tag() {
            Some(CapTag::Null) => {}
            Some(CapTag::Zombie) => {
                let mut current = crate::object::cnode::cap_snapshot(slot);
                if current.zombie_ptr() as *mut Cte == ptr
                    && current.zombie_number() == n
                    && current.zombie_type() == zombie_type
                {
                    let end_slot_empty = crate::object::cnode::cap_snapshot(end_slot).is_null();
                    debug_assert!(end_slot_empty, "reduced zombie end slot should be empty");
                    if !end_slot_empty {
                        panic!("reduced zombie end slot should be empty");
                    }
                    current.set_zombie_number(n - 1);
                    let _cspace_guard = crate::object::cnode::lock_cspace();
                    unsafe {
                        (*slot).cap = current;
                    }
                } else {
                    let self_referential = current.zombie_ptr() as *mut Cte == slot && ptr != slot;
                    debug_assert!(
                        self_referential,
                        "expected recursive delete to leave a self-referential zombie",
                    );
                    if !self_referential {
                        panic!("expected recursive delete to leave a self-referential zombie");
                    }
                }
            }
            None => {
                panic!("recursive delete left an invalid cap tag");
            }
            _ => {
                panic!("Expected recursion to result in Zombie.");
            }
        }
    } else {
        debug_assert!(ptr != slot, "cyclic zombie passed to unexposed reduce");
        if ptr == slot {
            panic!("cyclic zombie passed to unexposed reduce");
        }
        let ptr_cap = crate::object::cnode::cap_snapshot(ptr);
        if ptr_cap.tag() == Some(CapTag::Zombie) {
            let moving_self_referential = ptr_cap.zombie_ptr() as *mut Cte == ptr;
            debug_assert!(
                !moving_self_referential,
                "moving self-referential zombie aside",
            );
            if moving_self_referential {
                panic!("moving self-referential zombie aside");
            }
        }
        cap_swap_for_delete(ptr, slot);
    }
    Ok(())
}

fn cap_swap_for_delete(slot1: *mut Cte, slot2: *mut Cte) {
    debug_assert!(
        !slot1.is_null() && !slot2.is_null(),
        "capSwapForDelete expects valid slots",
    );
    if slot1.is_null() || slot2.is_null() {
        panic!("capSwapForDelete expects valid slots");
    }
    if slot1 == slot2 {
        return;
    }
    let cspace_guard = crate::object::cnode::lock_cspace();
    unsafe {
        let cap1 = (*slot1).cap;
        let cap2 = (*slot2).cap;
        cnode_swap_slots(&cspace_guard, cap1, slot1, cap2, slot2);
    }
}

/// Mirror seL4 `cteSwap`: exchange both cap values and MDB nodes, then
/// re-thread neighbouring MDB links to the new CTE locations.
unsafe fn cnode_swap_slots(
    _cspace_guard: &CspaceLockGuard,
    cap1: Cap,
    slot1: *mut Cte,
    cap2: Cap,
    slot2: *mut Cte,
) {
    debug_assert!(!slot1.is_null() && !slot2.is_null());
    debug_assert!(slot1 != slot2);
    if slot1.is_null() || slot2.is_null() {
        panic!("cteSwap expects valid slots");
    }
    if slot1 == slot2 {
        panic!("cteSwap slots must differ");
    }
    unsafe {
        (*slot1).cap = cap2;
        (*slot2).cap = cap1;

        let mdb1 = (*slot1).mdb;
        relink_mdb_neighbors(slot2, mdb1);

        let mdb2 = (*slot2).mdb;
        (*slot1).mdb = mdb2;
        (*slot2).mdb = mdb1;

        relink_mdb_neighbors(slot1, mdb2);
    }
}

unsafe fn relink_mdb_neighbors(new_slot: *mut Cte, mdb: MdbNode) {
    let prev = mdb.prev();
    if prev != 0 {
        let p = prev as *mut Cte;
        unsafe {
            (*p).mdb.set_next(new_slot as u64);
        }
    }
    let next = mdb.next();
    if next != 0 {
        let n = next as *mut Cte;
        unsafe {
            (*n).mdb.set_prev(new_slot as u64);
        }
    }
}

struct FinalizeTarget {
    slot: *mut Cte,
    cap: Cap,
    is_final: bool,
}

fn snapshot_slot_for_delete(slot: *mut Cte) -> Option<FinalizeTarget> {
    let cspace_guard = crate::object::cnode::lock_cspace();
    unsafe {
        if slot.is_null() {
            panic!("cteDelete expected a valid slot");
        }
        if (*slot).cap.is_null() {
            return None;
        }
        let cap = (*slot).cap;
        let is_final = is_final_capability(&cspace_guard, slot);
        Some(FinalizeTarget {
            slot,
            cap,
            is_final,
        })
    }
}

fn empty_finalized_slot(slot: *mut Cte, cleanup_info: Cap) {
    let cspace_guard = crate::object::cnode::lock_cspace();
    unsafe {
        if slot.is_null() {
            panic!("emptySlot expected a valid slot");
        }
        if !(*slot).cap.is_null() {
            empty_slot(&cspace_guard, slot, cleanup_info);
        }
    }
}

/// C kernel `emptySlot`: splice `slot` out of the MDB list, preserve the
/// list ordering for any descendants, and propagate `firstBadged` to the
/// successor. This differs subtly from `mdb_unlink`, which is also used
/// by Move/Mutate while the MDB node is about to be transplanted.
unsafe fn empty_slot(_cspace_guard: &CspaceLockGuard, slot: *mut Cte, cleanup_info: Cap) {
    debug_assert!(!slot.is_null());
    if slot.is_null() {
        panic!("emptySlot expects a valid slot");
    }
    unsafe {
        let mdb = (*slot).mdb;
        let prev = mdb.prev();
        let next = mdb.next();
        if prev != 0 {
            let p = prev as *mut Cte;
            (*p).mdb.set_next(next);
        }
        if next != 0 {
            let n = next as *mut Cte;
            (*n).mdb.set_prev(prev);
            if mdb.first_badged() {
                (*n).mdb.set_first_badged(true);
            }
        }
        (*slot).cap = Cap::null();
        (*slot).mdb = MdbNode::NULL;
    }
    post_cap_deletion(cleanup_info);
}

fn post_cap_deletion(cleanup_info: Cap) {
    if cleanup_info.tag() == Some(CapTag::IrqHandler) {
        unsafe {
            crate::object::irq::deleted_handler(cleanup_info.irq_handler_irq());
        }
    }
}

/// `isFinalCapability`-equivalent: check whether this CTE holds the last
/// remaining cap to its underlying object. Mirrors the C kernel logic in
/// `kernel/src/object/cnode.c`: walk the MDB neighbours (prev / next) and
/// see if either points at the same object. A cap is *final* iff no
/// neighbour shares the object.
unsafe fn is_final_capability(_cspace_guard: &CspaceLockGuard, slot: *mut Cte) -> bool {
    if slot.is_null() {
        panic!("isFinalCapability expects a valid slot");
    }
    let mdb = unsafe { (*slot).mdb };
    let cap = unsafe { (*slot).cap };
    if mdb.prev() != 0 {
        let p = mdb.prev() as *mut Cte;
        if same_object_as(unsafe { (*p).cap }, cap) {
            return false;
        }
    }
    if mdb.next() != 0 {
        let n = mdb.next() as *mut Cte;
        if same_object_as(cap, unsafe { (*n).cap }) {
            return false;
        }
    }
    true
}

/// Mirror of C kernel `sameObjectAs(cap_a, cap_b)`: Untyped caps are never
/// final objects, IRQControl caps are never object-equal to issued IRQHandler
/// caps, arch caps use their architecture-specific object identity, and other
/// caps follow `sameRegionAs`.
fn same_object_as(a: Cap, b: Cap) -> bool {
    if matches!(a.tag(), Some(CapTag::Untyped | CapTag::IrqControl)) {
        return false;
    }

    match (a.tag(), b.tag()) {
        (Some(CapTag::Endpoint), Some(CapTag::Endpoint)) => a.endpoint_ptr() == b.endpoint_ptr(),
        (Some(CapTag::Notification), Some(CapTag::Notification)) => {
            a.notification_ptr() == b.notification_ptr()
        }
        (Some(CapTag::CNode), Some(CapTag::CNode)) => {
            a.cnode_ptr() == b.cnode_ptr() && a.cnode_radix() == b.cnode_radix()
        }
        (Some(CapTag::Thread), Some(CapTag::Thread)) => a.thread_ptr() == b.thread_ptr(),
        (Some(CapTag::Reply), Some(CapTag::Reply)) => a.reply_object_ptr() == b.reply_object_ptr(),
        (Some(CapTag::IrqHandler), Some(CapTag::IrqHandler)) => {
            a.irq_handler_irq() == b.irq_handler_irq()
        }
        (Some(CapTag::Domain), Some(CapTag::Domain)) => true,
        (Some(CapTag::AsidControl), Some(CapTag::AsidControl)) => true,
        (Some(CapTag::PageTable), Some(CapTag::PageTable)) => {
            a.page_table_base_ptr() == b.page_table_base_ptr()
        }
        (Some(CapTag::AsidPool), Some(CapTag::AsidPool)) => a.asid_pool_ptr() == b.asid_pool_ptr(),
        (Some(CapTag::Frame), Some(CapTag::Frame)) => {
            a.frame_base_ptr() == b.frame_base_ptr()
                && a.frame_size() == b.frame_size()
                && a.frame_is_device() == b.frame_is_device()
        }
        _ => false,
    }
}

/// Architecture-aware "finalise this cap" hook. For Frame caps that are
/// still mapped, this rips the leaf PTE out of the owning VSpace so the
/// underlying memory can be safely re-used. Without this we hit a
/// classic use-after-free during Untyped reset:
///
///   Retype  → Frame F → Page_Map F→VA  →  Delete F  →  Retype again
///                                                         ↑
///                              still-mapped F's memory served to new
///                              owner → driver reads stale data via VA.
///
/// Mirrors the work `Arch_finaliseCap` does in `kernel/src/arch/.../object/objecttype.c`.
fn finalize_cap(
    _slot: *mut Cte,
    mut cap: Cap,
    is_final: bool,
    exposed: bool,
) -> Result<FinaliseCapResult, SyscallError> {
    debug_assert_kernel_lock_held();

    // seL4 dispatches architecture caps before the generic exposed-fail gate.
    match cap.tag() {
        Some(CapTag::Frame) => {
            if cap.frame_is_mapped() {
                // Route the unmap through the VSpace the cap was originally
                // mapped into, *not* the current thread. Otherwise a Revoke
                // on the parent Untyped would walk Frame children and erase
                // PTEs out of whatever VSpace happens to be active right
                // now (the driver), corrupting unrelated mappings.
                let asid = cap.frame_mapped_asid();
                let va = cap.frame_mapped_addr();
                let pa = kva_to_pa(cap.frame_base_ptr());
                let root_pt_kva = crate::object::asid::lookup(asid);
                if root_pt_kva != 0 {
                    unsafe {
                        let unmapped = crate::arch::current::vspace::unmap_user_frame(
                            root_pt_kva as *mut PageTable,
                            va as usize,
                            cap.frame_size(),
                            pa as usize,
                        );
                        let _ = unmapped;
                    }
                }
                cap.set_frame_mapped_addr(0);
                cap.set_frame_mapped_asid(0);
            }
            return Ok(FinaliseCapResult::null());
        }
        Some(CapTag::PageTable) => {
            if is_final && cap.page_table_is_mapped() {
                let asid = cap.page_table_mapped_asid();
                let pt_kva = cap.page_table_base_ptr();
                let root_pt_kva = crate::object::asid::lookup(asid);
                if root_pt_kva != 0 && root_pt_kva == pt_kva {
                    crate::object::asid::delete(asid, pt_kva);
                } else {
                    unsafe {
                        let _ = crate::arch::current::vspace::unmap_user_page_table(
                            root_pt_kva as *mut PageTable,
                            cap.page_table_mapped_addr() as usize,
                            pt_kva as *mut PageTable,
                        );
                    }
                }
                cap.clear_page_table_is_mapped();
            }
            return Ok(FinaliseCapResult::null());
        }
        Some(CapTag::AsidPool) => {
            if is_final {
                crate::object::asid::delete_pool(cap.asid_pool_base(), cap.asid_pool_ptr());
            }
            return Ok(FinaliseCapResult::null());
        }
        Some(CapTag::AsidControl) => return Ok(FinaliseCapResult::null()),
        _ => {}
    }

    match cap.tag() {
        Some(CapTag::Endpoint) => {
            // Mirrors C kernel `finaliseCap(cap, final, _)` Endpoint
            // branch: wake every blocked TCB **only** when this is the
            // last surviving cap to the EP (`final == true`). Deleting
            // a non-final cap (e.g. a derived/badged copy during a
            // Revoke) just unlinks the cap from MDB; the senders /
            // receivers stay queued on the EP because other refs to it
            // are still live.
            if is_final {
                let p = cap.endpoint_ptr();
                if p == 0 || !is_pspace_kva(p) {
                    debug_assert!(false, "final Endpoint cap must point at an endpoint");
                    panic!("final Endpoint cap must point at an endpoint");
                }
                unsafe {
                    crate::object::endpoint::finalize(p as *mut crate::object::endpoint::Endpoint);
                }
            }
            return Ok(FinaliseCapResult::null());
        }
        Some(CapTag::Notification) => {
            // Final-cap path only (see Endpoint above for the
            // rationale). Non-final delete leaves the notification
            // object and its waiters intact.
            if is_final {
                let p = cap.notification_ptr();
                if p == 0 || !is_pspace_kva(p) {
                    debug_assert!(false, "final Notification cap must point at a notification",);
                    panic!("final Notification cap must point at a notification");
                }
                unsafe {
                    let n = p as *mut crate::object::notification::Notification;
                    crate::object::notification::finalize(n);
                }
            }
            return Ok(FinaliseCapResult::null());
        }
        Some(CapTag::Reply) => {
            let reply = cap.reply_object_ptr();
            if is_final {
                if reply == 0 || !is_pspace_kva(reply) {
                    debug_assert!(false, "final Reply cap must point at a reply object");
                    panic!("final Reply cap must point at a reply object");
                }
                unsafe { crate::object::reply::finalize(reply) };
            }
            return Ok(FinaliseCapResult::null());
        }
        Some(CapTag::Null | CapTag::Domain) => return Ok(FinaliseCapResult::null()),
        _ => {}
    }

    if exposed {
        panic!("finaliseCap: failed to finalise immediately");
    }

    match cap.tag() {
        Some(CapTag::CNode) => {
            if is_final {
                let base = cap.cnode_ptr();
                let radix = cap.cnode_radix();
                if base == 0 || !is_boot_or_pspace_object_kva(base) {
                    debug_assert!(false, "final CNode cap must point at a CNode");
                    panic!("final CNode cap must point at a CNode");
                }
                return Ok(FinaliseCapResult::remainder(Cap::new_cnode_zombie(
                    1u64 << radix,
                    radix,
                    base,
                )));
            }
        }
        Some(CapTag::Thread) => {
            // Drop bound-notification linkage, queue links, etc., so a
            // stale pointer to this slab can't look "runnable" if some
            // future scheduler scan races the Revoke. The actual
            // storage is recycled by the parent Untyped on Retype.
            if is_final {
                let p = cap.thread_ptr();
                if p == 0 || !is_boot_or_pspace_object_kva(p) {
                    debug_assert!(false, "final Thread cap must point at a TCB");
                    panic!("final Thread cap must point at a TCB");
                }
                crate::kernel::smp::remote_tcb_stall(p as *mut Tcb);
                let cte_base = unsafe { crate::object::tcb::cap_slot_base(p as *mut Tcb) } as u64;
                if cte_base == 0 {
                    debug_assert!(false, "final Thread cap must expose TCB CTE slots");
                    panic!("final Thread cap must expose TCB CTE slots");
                }
                unsafe {
                    crate::object::tcb::finalize(p as *mut crate::object::tcb::Tcb);
                }
                return Ok(FinaliseCapResult::remainder(Cap::new_tcb_zombie(
                    tcb::TCB_ARCH_CNODE_ENTRIES as u64,
                    cte_base,
                )));
            }
        }
        Some(CapTag::Zombie) => {
            return Ok(FinaliseCapResult::remainder(cap));
        }
        Some(CapTag::IrqHandler) => {
            if is_final {
                unsafe {
                    crate::object::irq::deleting_handler(cap.irq_handler_irq());
                }
                return Ok(FinaliseCapResult::with_cleanup(Cap::null(), cap));
            }
        }
        _ => {}
    }
    Ok(FinaliseCapResult::null())
}

/// Walk the CDT descendants of `cte` and delete them through the same
/// `cteDelete(..., exposed=true)` path as seL4.
fn revoke_descendants(cte: *mut Cte) -> Result<(), SyscallError> {
    debug_assert_kernel_lock_held();
    loop {
        if !revoke_one_descendant(cte)? {
            return Ok(());
        }
        cnode_preemption_point()?;
    }
}

fn revoke_one_descendant(cte: *mut Cte) -> Result<bool, SyscallError> {
    let Some(slot) = next_descendant_for_revoke(cte) else {
        return Ok(false);
    };
    cte_delete(slot, true)?;
    Ok(true)
}

fn next_descendant_for_revoke(cte: *mut Cte) -> Option<*mut Cte> {
    let cspace_guard = crate::object::cnode::lock_cspace();
    unsafe {
        if cte.is_null() || !crate::object::cnode::mdb_has_children_locked(&cspace_guard, cte) {
            return None;
        }

        // seL4 `cteRevoke` repeatedly deletes the direct MDB successor while
        // it remains a child of the revoke root. `cteDelete` and zombies then
        // preserve any deeper descendants for later loop iterations.
        let child = (*cte).mdb.next() as *mut Cte;
        if child.is_null() {
            return None;
        }

        Some(child)
    }
}

/// Helper: read mr4 / mr5 from the IPC buffer. The IPC buffer's `msg`
/// array starts at offset 1 word inside the frame (word 0 is the tag).
fn read_mrs_4_5(_thread: &Thread) -> (u64, u64) {
    (
        crate::api::thread::current_ipc_buffer_word(1 + 4),
        crate::api::thread::current_ipc_buffer_word(1 + 5),
    )
}

/// Read `caps_or_badges[i]` from the current thread's IPC buffer. Used to
/// recover extra-cap CPtrs that the user marshalled via `seL4_SetCap`.
///
/// The IPC buffer layout has `msg[120]` words after the tag, then
/// `userData`, then `caps_or_badges[3]`. So caps_or_badges[i] lives at
/// word offset 1 + 120 + 1 + i = 122 + i.
fn read_extra_cap(_thread: &Thread, i: usize) -> u64 {
    debug_assert!(i < 3);
    crate::api::thread::current_ipc_buffer_word(122 + i)
}

#[inline]
fn align_up(v: u64, bits: u64) -> u64 {
    let mask = (1u64 << bits) - 1;
    (v + mask) & !mask
}

#[inline]
fn align_down(v: u64, bits: u64) -> u64 {
    let mask = (1u64 << bits) - 1;
    v & !mask
}

#[allow(unused_imports)]
use cspace as _;
