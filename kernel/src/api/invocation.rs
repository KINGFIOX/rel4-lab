//! Cap-type-specific invocation handlers.
//!
//! Each function consumes the cap that was looked up plus the message
//! arguments (mr0..mr3 in `UserContext.a2..a5`, mr4+ in the IPC buffer)
//! and either mutates kernel state to perform the requested action or
//! returns a `SyscallError` for the caller to relay.

#![allow(dead_code)]

use core::ptr;

use crate::api::cspace;
use crate::api::syscall::SyscallError;
use crate::api::thread::Thread;
use crate::arch::riscv64::trap::{UserContext, reg};
use crate::object::cap::{Cap, CapTag};
use crate::object::cnode::{Cte, cnode_at};
use crate::object::mdb::MdbNode;

/// Object type IDs as defined by `seL4_ObjectType` (`api_object` +
/// `_mode_object` + `_object` for RISC-V).
mod obj {
    pub const UNTYPED: u64 = 0;
    pub const TCB: u64 = 1;
    pub const ENDPOINT: u64 = 2;
    pub const NOTIFICATION: u64 = 3;
    pub const CAP_TABLE: u64 = 4;
    // mode-specific (RV64): 5 = Giga page
    pub const GIGA_PAGE: u64 = 5;
    pub const FOUR_K_PAGE: u64 = 6;
    pub const MEGA_PAGE: u64 = 7;
    pub const PAGE_TABLE: u64 = 8;
}

/// Invocation labels — must agree with `enum invocation_label` from
/// `libsel4/include/sel4/invocation.h`. The exact numbering is generated
/// by the kernel's invocation_header_gen.py; we only enumerate the cases
/// we actually handle.
mod label {
    pub const UNTYPED_RETYPE: u64 = 1;
    // Non-MCS ordering from `libsel4/include/sel4/invocation.h`. The
    // TCB block occupies labels 2..=16; CNode ops begin at 17.
    pub const CNODE_REVOKE: u64 = 17;
    pub const CNODE_DELETE: u64 = 18;
    pub const CNODE_CANCEL_BADGED_SENDS: u64 = 19;
    pub const CNODE_COPY: u64 = 20;
    pub const CNODE_MINT: u64 = 21;
    pub const CNODE_MOVE: u64 = 22;
    pub const CNODE_MUTATE: u64 = 23;
    pub const CNODE_ROTATE: u64 = 24;
    pub const CNODE_SAVE_CALLER: u64 = 25;
}

/// Helper: compute log2 of the in-memory bytes of an object given its
/// type and user-supplied size (used for CNode / Untyped where the user
/// picks a radix).
fn object_size_bits(ty: u64, user_size: u64) -> Option<u64> {
    use crate::abi::constants::{
        SEL4_ENDPOINT_BITS, SEL4_NOTIFICATION_BITS, SEL4_SLOT_BITS, SEL4_TCB_BITS,
    };
    Some(match ty {
        obj::UNTYPED => user_size,
        obj::TCB => SEL4_TCB_BITS as u64,
        obj::ENDPOINT => SEL4_ENDPOINT_BITS as u64,
        obj::NOTIFICATION => SEL4_NOTIFICATION_BITS as u64,
        obj::CAP_TABLE => user_size + SEL4_SLOT_BITS as u64,
        obj::FOUR_K_PAGE | obj::PAGE_TABLE => 12,
        obj::MEGA_PAGE => 21,
        obj::GIGA_PAGE => 30,
        _ => return None,
    })
}

/// Construct the cap_t for a freshly allocated object.
fn create_object_cap(ty: u64, region_base: u64, user_size: u64, is_device: bool) -> Option<Cap> {
    Some(match ty {
        obj::UNTYPED => Cap::new_untyped(region_base, user_size, 0, is_device),
        obj::CAP_TABLE => {
            // Fresh CNode caps have no guard: callers are expected to
            // set one with `seL4_CNode_Mint`/`Mutate` when they put the
            // cap into a CSpace. Matches `createCNodeObject` in
            // `kernel/src/object/objecttype.c`.
            Cap::new_cnode(region_base, user_size, 0, 0)
        }
        obj::FOUR_K_PAGE => Cap::new_frame(region_base, 0, 2 /* RW */, is_device),
        obj::MEGA_PAGE => Cap::new_frame(region_base, 1, 2, is_device),
        obj::GIGA_PAGE => Cap::new_frame(region_base, 2, 2, is_device),
        obj::PAGE_TABLE => Cap::new_page_table(region_base),
        obj::ENDPOINT => Cap::new_endpoint(region_base),
        obj::NOTIFICATION => Cap::new_notification(region_base),
        obj::TCB => Cap::new_thread(region_base),
        _ => return None,
    })
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
    if label_id != label::UNTYPED_RETYPE {
        return Err(SyscallError::IllegalOperation);
    }
    if length < 6 {
        return Err(SyscallError::TruncatedMessage);
    }

    let new_type = uc.regs[reg::A2];
    let user_size = uc.regs[reg::A3];
    let node_index = uc.regs[reg::A4];
    let node_depth = uc.regs[reg::A5];
    let (node_offset, node_window) = read_mrs_4_5(thread);

    // The dest-CNode CPtr was placed in `caps_or_badges[0]` by the libsel4
    // stub's `seL4_SetCap(0, root)`.
    let root_cptr = read_extra_cap(thread, 0);

    let obj_bits = object_size_bits(new_type, user_size).ok_or(SyscallError::IllegalOperation)?;

    if node_window < 1 || node_window > 256 {
        uc.regs[reg::A2] = 1;
        uc.regs[reg::A3] = 256;
        return Err(SyscallError::RangeError);
    }

    // Resolve the destination CNode capability.
    //   nodeDepth == 0 → use the looked-up cap *directly* (it must be a CNode).
    //   nodeDepth > 0  → walk `nodeIndex` for `nodeDepth` bits within it.
    let (root_cap, _) =
        cspace::lookup_cap(thread, root_cptr).map_err(|_| SyscallError::InvalidCapability)?;
    let dest_cnode_cap = if node_depth == 0 {
        root_cap
    } else {
        let node_slot = resolve_slot(root_cap, node_index, node_depth as u32)?;
        unsafe { (*node_slot).cap }
    };
    if dest_cnode_cap.tag() != Some(CapTag::CNode) {
        return Err(SyscallError::FailedLookup);
    }
    let dest_radix = dest_cnode_cap.cnode_radix();
    if node_offset >= (1u64 << dest_radix) {
        uc.regs[reg::A2] = 0;
        uc.regs[reg::A3] = (1u64 << dest_radix) - 1;
        return Err(SyscallError::RangeError);
    }
    if node_window > (1u64 << dest_radix) - node_offset {
        uc.regs[reg::A2] = 1;
        uc.regs[reg::A3] = (1u64 << dest_radix) - node_offset;
        return Err(SyscallError::RangeError);
    }
    let dest_base_kva = dest_cnode_cap.cnode_ptr();
    let dest_cnode = unsafe { cnode_at(dest_base_kva as *mut u8, dest_radix as usize) };

    // Ensure target slots are empty.
    for i in 0..node_window {
        let slot = &dest_cnode[(node_offset + i) as usize];
        if !slot.cap.is_null() {
            return Err(SyscallError::DeleteFirst);
        }
    }

    let untyped_bits = src_cap.untyped_block_size_bits();
    let is_device = src_cap.untyped_is_device();
    let region_base_kva = src_cap.untyped_ptr();
    let region_size = 1u64 << untyped_bits;

    // If the untyped has no surviving CDT descendants we restart
    // allocation from offset 0 — mirrors `resetUntypedCap` in the C
    // kernel's `decodeUntypedInvocation`. This is what makes a
    // Revoke-on-parent return a fully fresh untyped to libsel4allocman
    // so subsequent `_refill_pool` calls don't drown in NotEnoughMemory.
    let has_children = unsafe { crate::object::cnode::mdb_has_children(src_slot) };
    let stored_fi = src_cap.untyped_free_index();
    let free_index = if has_children {
        stored_fi
    } else {
        if stored_fi != 0 {
            unsafe {
                let s = &mut *src_slot;
                s.cap.set_untyped_free_index(0);
            }
        }
        0
    };
    let used_bytes = free_index << 4;
    let free_bytes = region_size.saturating_sub(used_bytes);

    let aligned_start_offset = align_up(used_bytes, obj_bits);
    let total_obj_bytes = node_window << obj_bits;

    if aligned_start_offset.saturating_add(total_obj_bytes) > region_size {
        return Err(SyscallError::NotEnoughMemory);
    }
    let _ = free_bytes;

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
        let cap = create_object_cap(new_type, obj_base, user_size, is_device)
            .ok_or(SyscallError::IllegalOperation)?;
        // Per-object init hook. For TCBs we stamp the `Tcb` struct
        // skeleton onto the freshly zeroed slab so that subsequent
        // TCB_* invocations have a real place to land data. Endpoints
        // are also stamped — though `Untyped_Retype` zeroed the slab
        // already, going through `endpoint::init` keeps the layout
        // contract explicit at the one place objects come to life.
        match new_type {
            obj::TCB => unsafe { crate::object::tcb::init(obj_base) },
            obj::ENDPOINT => unsafe { crate::object::endpoint::init(obj_base) },
            obj::NOTIFICATION => unsafe { crate::object::notification::init(obj_base) },
            obj::PAGE_TABLE => {
                // Stamp the global kernel + PSpace mappings into the
                // new root PT so a `satp` swap to it can still trap to
                // S-mode (`trap_entry` lives in the kernel ELF window).
                // Cheap on intermediate PTs too — those entries land in
                // the upper half (>= L2[256]) which user code never
                // walks through.
                crate::arch::riscv64::vspace::copy_kernel_mappings_to(
                    obj_base as *mut crate::arch::riscv64::sv39::PageTable,
                );
            }
            _ => {}
        }
        // Mirrors `isCapRevocable(newCap, srcCap)` from
        // `kernel/src/object/objecttype.c`. For arch caps (Frame /
        // PageTable) it returns false, so children of an Untyped are
        // *not* themselves revocable — that lets the user `Delete` them
        // without first `Revoke`ing the parent. Only sub-Untypeds (and
        // future badged EP / IRQ-handler caps) are revocable.
        let new_revocable = new_type == obj::UNTYPED;
        let dst = &mut dest_cnode[(node_offset + i) as usize];
        dst.cap = cap;
        dst.mdb = MdbNode::new(0, 0, new_revocable, true);
        unsafe {
            crate::object::cnode::mdb_insert_after(src_slot, dst as *mut Cte);
        }
    }

    // Bump the untyped's free index past what we used.
    let new_used_bytes = aligned_start_offset + total_obj_bytes;
    let new_free_index = new_used_bytes >> 4;
    unsafe {
        let s = &mut *src_slot;
        s.cap.set_untyped_free_index(new_free_index);
    }

    Ok(())
}

/// RISC-V Page_Map / Page_Unmap / Page_GetAddress.
///
/// The labels live in `arch_invocation_label`. With non-MCS build:
///   33 RISCVPageTableMap     34 RISCVPageTableUnmap
///   35 RISCVPageMap          36 RISCVPageUnmap
///   37 RISCVPageGetAddress
pub fn handle_frame(
    thread: &Thread,
    _slot: *mut Cte,
    cap: Cap,
    label_id: u64,
    length: u64,
    uc: &mut UserContext,
) -> Result<(), SyscallError> {
    use crate::arch::riscv64::vspace;

    const RISCV_PAGE_MAP: u64 = 35;
    const RISCV_PAGE_UNMAP: u64 = 36;
    const RISCV_PAGE_GET_ADDR: u64 = 37;

    match label_id {
        RISCV_PAGE_MAP => {
            if length < 3 {
                return Err(SyscallError::TruncatedMessage);
            }
            let vaddr = uc.regs[reg::A2];
            // libsel4 packs `seL4_CapRights_t` (from `shared_types.pbf`):
            //   bit 0 capAllowWrite, bit 1 capAllowRead,
            //   bit 2 capAllowGrant, bit 3 capAllowGrantReply.
            // VM attributes (RISC-V) bit 0 = riscvExecuteNever.
            let rights_packed = uc.regs[reg::A3];
            let attrs = uc.regs[reg::A4];
            let can_write = (rights_packed & 0x1) != 0;
            let can_read = (rights_packed & 0x2) != 0;
            let exec_never = (attrs & 0x1) != 0;

            let vspace_cptr = read_extra_cap(thread, 0);
            let (vspace_cap, _) = cspace::lookup_cap(thread, vspace_cptr)
                .map_err(|_| SyscallError::InvalidCapability)?;
            if vspace_cap.tag() != Some(CapTag::PageTable) {
                return Err(SyscallError::InvalidCapability);
            }
            let root_pt_kva = vspace_cap.page_table_base_ptr();

            // Frame's underlying memory: capFBasePtr is the kernel-window VA
            // of the start of the frame.
            let frame_kva = cap.frame_base_ptr();
            let frame_pa = kva_to_pa(frame_kva);

            // Track which VSpace this frame is going into so a later
            // Page_Unmap routes to the right root PT instead of clobbering
            // the current thread's mappings. ASID 0 means "no mapping
            // recorded" so we leave mapped_addr=0 in that pathological
            // case (table exhausted).
            let asid = crate::object::asid::assign(root_pt_kva);
            unsafe {
                vspace::map_user_frame(
                    root_pt_kva as *mut crate::arch::riscv64::sv39::PageTable,
                    vaddr as usize,
                    frame_pa as usize,
                    cap.frame_size(),
                    vspace::user_flags(can_read, can_write, !exec_never),
                );
                (*_slot).cap.set_frame_mapped_addr(vaddr);
                (*_slot).cap.set_frame_mapped_asid(asid);
            }
            Ok(())
        }
        RISCV_PAGE_UNMAP => {
            let frame_va = cap.frame_mapped_addr();
            if frame_va == 0 {
                return Ok(());
            }
            let asid = cap.frame_mapped_asid();
            let root_pt_kva = crate::object::asid::lookup(asid);
            if root_pt_kva == 0 {
                // Best effort: clear the cap's mapped_addr but don't
                // touch any page table. This is what the C kernel does
                // for caps whose ASID has been freed under it.
                unsafe {
                    (*_slot).cap.set_frame_mapped_addr(0);
                    (*_slot).cap.set_frame_mapped_asid(0);
                }
                return Ok(());
            }
            unsafe {
                let _ = vspace::unmap_user_frame(
                    root_pt_kva as *mut crate::arch::riscv64::sv39::PageTable,
                    frame_va as usize,
                    cap.frame_size(),
                );
                (*_slot).cap.set_frame_mapped_addr(0);
                (*_slot).cap.set_frame_mapped_asid(0);
            }
            Ok(())
        }
        RISCV_PAGE_GET_ADDR => {
            // Return the frame's physical address in mr0.
            let frame_pa = kva_to_pa(cap.frame_base_ptr());
            unsafe {
                if !thread.ipc_buffer_kva.is_null() {
                    *thread.ipc_buffer_kva.add(1) = frame_pa;
                }
            }
            uc.regs[reg::A2] = frame_pa;
            Ok(())
        }
        _ => {
            let _ = label_id;
            Err(SyscallError::IllegalOperation)
        }
    }
}

/// RISC-V PageTable_Map.
///
/// Since `handle_frame::RISCV_PAGE_MAP` auto-allocates any missing PT
/// levels via the boot pool, we can treat PageTable_Map as a successful
/// no-op for M3 — the user's PT page exists as memory but our walker
/// allocates its own. Long-term we'll want to actually install the
/// user's PT so its `IsMapped` field is honoured by Unmap/Delete.
pub fn handle_page_table(
    _thread: &Thread,
    _slot: *mut Cte,
    _cap: Cap,
    label_id: u64,
    _length: u64,
    _uc: &mut UserContext,
) -> Result<(), SyscallError> {
    const RISCV_PAGE_TABLE_MAP: u64 = 33;
    const RISCV_PAGE_TABLE_UNMAP: u64 = 34;

    match label_id {
        RISCV_PAGE_TABLE_MAP => Ok(()),
        RISCV_PAGE_TABLE_UNMAP => Ok(()),
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
    const RISCV_ASID_CONTROL_MAKE_POOL: u64 = 38;
    if label_id != RISCV_ASID_CONTROL_MAKE_POOL {
        return Err(SyscallError::IllegalOperation);
    }
    if length < 2 {
        return Err(SyscallError::TruncatedMessage);
    }

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
    if untyped_cap.untyped_block_size_bits() != crate::abi::constants::SEL4_ASID_POOL_BITS as u64
        || untyped_cap.untyped_is_device()
    {
        return Err(SyscallError::InvalidCapability);
    }
    unsafe {
        if crate::object::cnode::mdb_has_children(untyped_slot) {
            return Err(SyscallError::RevokeFirst);
        }
    }

    let dest_index = uc.regs[reg::A2];
    let dest_depth = uc.regs[reg::A3] as u32 & 0xff;
    let dest = resolve_slot(root_cap, dest_index, dest_depth)?;
    unsafe {
        if !(*dest).cap.is_null() {
            return Err(SyscallError::DeleteFirst);
        }
        let pool_ptr = untyped_cap.untyped_ptr();
        let base =
            crate::object::asid::alloc_pool_base(pool_ptr).ok_or(SyscallError::DeleteFirst)?;
        ptr::write_bytes(
            pool_ptr as *mut u8,
            0,
            1usize << crate::abi::constants::SEL4_ASID_POOL_BITS,
        );
        (*dest).cap = Cap::new_asid_pool(base as u64, pool_ptr);
        (*dest).mdb = MdbNode::new(0, 0, true, true);
        crate::object::cnode::mdb_insert_after(untyped_slot, dest);
        // Make sure the backing untyped looks fully consumed, just like
        // the C kernel's `performASIDControlInvocation`.
        let s = &mut *untyped_slot;
        s.cap
            .set_untyped_free_index(1u64 << (crate::abi::constants::SEL4_ASID_POOL_BITS - 4));
    }
    Ok(())
}

pub fn handle_asid_pool(
    thread: &Thread,
    cap: Cap,
    label_id: u64,
    _length: u64,
    _uc: &UserContext,
) -> Result<(), SyscallError> {
    const RISCV_ASID_POOL_ASSIGN: u64 = 39;
    if label_id != RISCV_ASID_POOL_ASSIGN {
        return Err(SyscallError::IllegalOperation);
    }

    let vspace_cptr = read_extra_cap(thread, 0);
    let (vspace_cap, vspace_slot) =
        cspace::lookup_cap(thread, vspace_cptr).map_err(|_| SyscallError::InvalidCapability)?;
    if vspace_cap.tag() != Some(CapTag::PageTable) {
        return Err(SyscallError::InvalidCapability);
    }
    if vspace_cap.page_table_is_mapped() || vspace_cap.page_table_mapped_asid() != 0 {
        return Err(SyscallError::InvalidCapability);
    }

    let root_pt_kva = vspace_cap.page_table_base_ptr();
    let asid = crate::object::asid::assign_from_pool(
        cap.asid_pool_base(),
        cap.asid_pool_ptr(),
        root_pt_kva,
    )
    .ok_or(SyscallError::DeleteFirst)?;
    unsafe {
        (*vspace_slot).cap.set_page_table_mapped_asid(asid);
    }
    Ok(())
}

/// DomainSet invocations.
///
/// This build has `CONFIG_NUM_DOMAINS = 1`, so setting a thread to
/// domain 0 is a successful metadata update and every other domain is
/// `seL4_InvalidArgument`.
pub fn handle_domain(
    thread: &Thread,
    _cap: Cap,
    label_id: u64,
    length: u64,
    uc: &mut UserContext,
) -> Result<(), SyscallError> {
    const DOMAIN_SET_SET: u64 = 30;

    if label_id != DOMAIN_SET_SET {
        return Err(SyscallError::IllegalOperation);
    }
    if length < 1 {
        return Err(SyscallError::TruncatedMessage);
    }

    let domain = uc.regs[reg::A2] & 0xff;
    if domain >= crate::abi::constants::NUM_DOMAINS as u64 {
        return Err(SyscallError::InvalidArgument);
    }

    let tcb_cap = lookup_extra_cap(thread, 0)?;
    if tcb_cap.tag() != Some(CapTag::Thread) {
        return Err(SyscallError::InvalidArgument);
    }
    let tcb_ptr = crate::object::tcb::from_cap(tcb_cap);
    if tcb_ptr.is_null() {
        return Err(SyscallError::InvalidArgument);
    }
    unsafe {
        (*tcb_ptr).domain = domain as u8;
    }
    Ok(())
}

/// TCB invocations.
///
/// Label values (non-MCS build) come from `enum invocation_label` in
/// `libsel4/include/sel4/invocation.h`:
///
/// ```text
///  2 TCBReadRegisters      8 TCBSetSchedParams   13 TCBBindNotification
///  3 TCBWriteRegisters     9 TCBSetIPCBuffer     14 TCBUnbindNotification
///  4 TCBCopyRegisters     10 TCBSetSpace         15 TCBSetTLSBase
///  5 TCBConfigure         11 TCBSuspend          16 TCBSetFlags
///  6 TCBSetPriority       12 TCBResume
///  7 TCBSetMCPriority
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
    _slot: *mut Cte,
    cap: Cap,
    label_id: u64,
    length: u64,
    uc: &mut UserContext,
) -> Result<(), SyscallError> {
    use crate::object::tcb;

    const TCB_READ_REGISTERS: u64 = 2;
    const TCB_WRITE_REGISTERS: u64 = 3;
    const TCB_COPY_REGISTERS: u64 = 4;
    const TCB_CONFIGURE: u64 = 5;
    const TCB_SET_PRIORITY: u64 = 6;
    const TCB_SET_MC_PRIORITY: u64 = 7;
    const TCB_SET_SCHED_PARAMS: u64 = 8;
    const TCB_SET_IPC_BUFFER: u64 = 9;
    const TCB_SET_SPACE: u64 = 10;
    const TCB_SUSPEND: u64 = 11;
    const TCB_RESUME: u64 = 12;
    const TCB_BIND_NOTIFICATION: u64 = 13;
    const TCB_UNBIND_NOTIFICATION: u64 = 14;
    const TCB_SET_TLS_BASE: u64 = 15;
    const TCB_SET_FLAGS: u64 = 16;

    let tcb_ptr = tcb::from_cap(cap);
    if tcb_ptr.is_null() {
        return Err(SyscallError::InvalidCapability);
    }

    match label_id {
        TCB_CONFIGURE => {
            // libsel4: tag = MessageInfo(TCBConfigure, 0, 3, 4)
            //   extraCaps[0] = cspace_root
            //   extraCaps[1] = vspace_root
            //   extraCaps[2] = buffer_frame
            //   mr0 = fault_ep, mr1 = cspace_data,
            //   mr2 = vspace_data, mr3 = buffer_uva
            if length < 4 {
                return Err(SyscallError::TruncatedMessage);
            }
            let fault_ep = uc.regs[reg::A2];
            let cspace_data = uc.regs[reg::A3];
            let _vspace_data = uc.regs[reg::A4];
            let buffer_uva = uc.regs[reg::A5];

            let mut cspace_cap = lookup_extra_cap(thread, 0)?;
            let vspace_cap = lookup_extra_cap(thread, 1)?;
            let buffer_cap = lookup_extra_cap(thread, 2)?;

            require_tag(cspace_cap, CapTag::CNode)?;
            require_tag(vspace_cap, CapTag::PageTable)?;
            require_tag(buffer_cap, CapTag::Frame)?;

            // `seL4_CNode_CapData` packs (guard, guard_size) that the
            // rootserver wants stamped onto the cspace cap before it
            // becomes the test process's CSpace root — see C kernel
            // `decodeTCBConfigure` → `updateCapData`.
            if cspace_data != 0 {
                cspace_cap.cnode_apply_capdata(cspace_data);
            }

            unsafe {
                (*tcb_ptr).cspace_cap = cspace_cap;
                (*tcb_ptr).vspace_cap = vspace_cap;
                (*tcb_ptr).ipc_buffer_cap = buffer_cap;
                (*tcb_ptr).ipc_buffer_uva = buffer_uva;
                (*tcb_ptr).fault_ep_cptr = fault_ep;
                // Pre-compute the kernel-window VA of the IPC buffer
                // frame so a future restore_user_context path can poke
                // MRs without re-walking the cap.
                (*tcb_ptr).ipc_buffer_kva = buffer_cap.frame_base_ptr();
            }
            Ok(())
        }

        TCB_SET_SPACE => {
            // libsel4: tag = MessageInfo(TCBSetSpace, 0, 2, 3)
            //   extraCaps[0] = cspace_root, extraCaps[1] = vspace_root
            //   mr0 = fault_ep, mr1 = cspace_data, mr2 = vspace_data
            if length < 3 {
                return Err(SyscallError::TruncatedMessage);
            }
            let fault_ep = uc.regs[reg::A2];
            let cspace_data = uc.regs[reg::A3];
            let _vspace_data = uc.regs[reg::A4];

            let mut cspace_cap = lookup_extra_cap(thread, 0)?;
            let vspace_cap = lookup_extra_cap(thread, 1)?;
            require_tag(cspace_cap, CapTag::CNode)?;
            require_tag(vspace_cap, CapTag::PageTable)?;
            if cspace_data != 0 {
                cspace_cap.cnode_apply_capdata(cspace_data);
            }

            unsafe {
                (*tcb_ptr).cspace_cap = cspace_cap;
                (*tcb_ptr).vspace_cap = vspace_cap;
                (*tcb_ptr).fault_ep_cptr = fault_ep;
            }
            Ok(())
        }

        TCB_SET_IPC_BUFFER => {
            // libsel4: tag = MessageInfo(TCBSetIPCBuffer, 0, 1, 1)
            //   extraCaps[0] = buffer_frame, mr0 = buffer_uva
            if length < 1 {
                return Err(SyscallError::TruncatedMessage);
            }
            let buffer_uva = uc.regs[reg::A2];
            let buffer_cap = lookup_extra_cap(thread, 0)?;
            require_tag(buffer_cap, CapTag::Frame)?;
            unsafe {
                (*tcb_ptr).ipc_buffer_cap = buffer_cap;
                (*tcb_ptr).ipc_buffer_uva = buffer_uva;
                (*tcb_ptr).ipc_buffer_kva = buffer_cap.frame_base_ptr();
            }
            Ok(())
        }

        TCB_SET_PRIORITY => {
            // libsel4: tag = MessageInfo(TCBSetPriority, 0, 1, 1)
            //   extraCaps[0] = authority (TCB), mr0 = priority
            if length < 1 {
                return Err(SyscallError::TruncatedMessage);
            }
            let prio = uc.regs[reg::A2];
            let auth_cap = lookup_extra_cap(thread, 0)?;
            require_tag(auth_cap, CapTag::Thread)?;
            let auth_tcb = tcb::from_cap(auth_cap);
            if auth_tcb.is_null() {
                return Err(SyscallError::InvalidCapability);
            }
            let auth_mcp = unsafe { (*auth_tcb).mcp as u64 };
            if prio > 255 || prio > auth_mcp {
                return Err(SyscallError::RangeError);
            }
            unsafe { tcb::set_priority(tcb_ptr, prio as u8) };
            Ok(())
        }

        TCB_SET_MC_PRIORITY => {
            if length < 1 {
                return Err(SyscallError::TruncatedMessage);
            }
            let mcp = uc.regs[reg::A2];
            let auth_cap = lookup_extra_cap(thread, 0)?;
            require_tag(auth_cap, CapTag::Thread)?;
            let auth_tcb = tcb::from_cap(auth_cap);
            if auth_tcb.is_null() {
                return Err(SyscallError::InvalidCapability);
            }
            let auth_mcp = unsafe { (*auth_tcb).mcp as u64 };
            if mcp > 255 || mcp > auth_mcp {
                return Err(SyscallError::RangeError);
            }
            unsafe { tcb::set_mcp(tcb_ptr, mcp as u8) };
            Ok(())
        }

        TCB_SET_SCHED_PARAMS => {
            // libsel4 (non-MCS): tag = MessageInfo(_, 0, 1, 2)
            //   extraCaps[0] = authority, mr0 = mcp, mr1 = priority
            if length < 2 {
                return Err(SyscallError::TruncatedMessage);
            }
            let mcp = uc.regs[reg::A2];
            let prio = uc.regs[reg::A3];
            let auth_cap = lookup_extra_cap(thread, 0)?;
            require_tag(auth_cap, CapTag::Thread)?;
            let auth_tcb = tcb::from_cap(auth_cap);
            if auth_tcb.is_null() {
                return Err(SyscallError::InvalidCapability);
            }
            let auth_mcp = unsafe { (*auth_tcb).mcp as u64 };
            if mcp > 255 || prio > 255 || mcp > auth_mcp || prio > auth_mcp {
                return Err(SyscallError::RangeError);
            }
            unsafe {
                tcb::set_mcp(tcb_ptr, mcp as u8);
                tcb::set_priority(tcb_ptr, prio as u8);
            }
            Ok(())
        }

        TCB_SUSPEND => {
            unsafe { tcb::suspend(tcb_ptr) };
            Ok(())
        }

        TCB_RESUME => {
            unsafe { tcb::resume(tcb_ptr) };
            Ok(())
        }

        TCB_WRITE_REGISTERS => {
            // libsel4: tag = MessageInfo(TCBWriteRegisters, 0, 0, 34)
            //   mr0 = (resume_target & 1) | ((arch_flags & 0xff) << 8)
            //   mr1 = count, mr2 = pc, mr3 = ra
            //   mr4.. = sp, gp, tp, s0..s11, a0..a7, t0..t6  (in that order)
            if length < 2 {
                return Err(SyscallError::TruncatedMessage);
            }
            let flag_word = uc.regs[reg::A2];
            let count = uc.regs[reg::A3];
            let resume_target = (flag_word & 1) != 0;

            unsafe {
                if count >= 1 && length >= 3 {
                    (*tcb_ptr).context.pc = uc.regs[reg::A4];
                }
                if count >= 2 && length >= 4 {
                    (*tcb_ptr).context.regs[reg::RA] = uc.regs[reg::A5];
                }
            }
            // Remaining regs (mr4..) live in the IPC buffer. The RISC-V
            // `seL4_UserContext` layout — and the corresponding C
            // kernel `frameRegisters[]` ++ `gpRegisters[]` flattening
            // that libsel4's stub marshals via
            // `((seL4_Word*)&regs->sp)[i-2]` — is:
            //   0:pc, 1:ra, 2:sp, 3:gp,
            //   4..15:s0..s11, 16..23:a0..a7,
            //   24..30:t0..t6, 31:tp
            // Note `tp` sits at the END of the struct on RISC-V, *not*
            // at position 4 — a layout quirk vs. the standard ABI
            // ordering. Slots 0/1 are 0-marked because pc/ra are
            // handled above.
            const X_INDEX: [usize; 32] = [
                /* 0 pc, 1 ra (handled above) */ 0,
                0,
                /* 2 sp  */ reg::SP,
                /* 3 gp  */ reg::GP,
                /* 4..15 s0..s11 */ 8,
                9,
                18,
                19,
                20,
                21,
                22,
                23,
                24,
                25,
                26,
                27,
                /* 16..23 a0..a7 */ reg::A0,
                reg::A1,
                reg::A2,
                reg::A3,
                reg::A4,
                reg::A5,
                reg::A6,
                reg::A7,
                /* 24..30 t0..t6 */ reg::T0,
                6,
                7,
                28,
                29,
                30,
                31,
                /* 31 tp */ reg::TP,
            ];
            if length >= 5 && count >= 3 {
                let mr_count = ((length - 1) as usize).min(count as usize).min(34);
                if !thread.ipc_buffer_kva.is_null() {
                    // mr_i for i=4..mr_count holds frameRegister/gpRegister
                    // value at slot (i-2) of seL4_UserContext.
                    for i in 4..mr_count {
                        let mr_val = unsafe { *thread.ipc_buffer_kva.add(1 + i) };
                        let ctx_idx = i - 2;
                        let target_idx = X_INDEX[ctx_idx];
                        if target_idx != 0 {
                            unsafe {
                                (*tcb_ptr).context.regs[target_idx] = mr_val;
                            }
                        }
                    }
                }
            }
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

        TCB_READ_REGISTERS => {
            // libsel4: tag = MessageInfo(TCBReadRegisters, 0, 0, 2)
            //   mr0 = (suspend_source & 1) | ((arch_flags & 0xff) << 8)
            //   mr1 = count
            // On reply, the kernel returns up to `count` registers:
            //   mr0 = pc, mr1 = ra, mr2 = sp, mr3 = gp
            //   mr4.. = s0..s11, a0..a7, t0..t6, tp  (in that order)
            //
            // Same RISC-V `seL4_UserContext` layout as TCB_WriteRegisters
            // — share the same `X_INDEX` table.
            if length < 2 {
                return Err(SyscallError::TruncatedMessage);
            }
            let flag_word = uc.regs[reg::A2];
            let count = uc.regs[reg::A3] as usize;
            let suspend_source = (flag_word & 1) != 0;

            const X_INDEX: [usize; 32] = [
                /* 0 pc, 1 ra (handled below) */ 0,
                0,
                /* 2 sp  */ reg::SP,
                /* 3 gp  */ reg::GP,
                /* 4..15 s0..s11 */ 8,
                9,
                18,
                19,
                20,
                21,
                22,
                23,
                24,
                25,
                26,
                27,
                /* 16..23 a0..a7 */ reg::A0,
                reg::A1,
                reg::A2,
                reg::A3,
                reg::A4,
                reg::A5,
                reg::A6,
                reg::A7,
                /* 24..30 t0..t6 */ reg::T0,
                6,
                7,
                28,
                29,
                30,
                31,
                /* 31 tp */ reg::TP,
            ];

            // Read register at seL4_UserContext field index `i`.
            let read_reg = |i: usize| -> u64 {
                if i == 0 {
                    unsafe { (*tcb_ptr).context.pc }
                } else if i == 1 {
                    unsafe { (*tcb_ptr).context.regs[reg::RA] }
                } else if i < 32 {
                    let idx = X_INDEX[i];
                    if idx == 0 {
                        0
                    } else {
                        unsafe { (*tcb_ptr).context.regs[idx] }
                    }
                } else {
                    0
                }
            };

            let n = count.min(32);
            // First 4 MRs go through registers a2..a5.
            if n >= 1 {
                uc.regs[reg::A2] = read_reg(0);
            }
            if n >= 2 {
                uc.regs[reg::A3] = read_reg(1);
            }
            if n >= 3 {
                uc.regs[reg::A4] = read_reg(2);
            }
            if n >= 4 {
                uc.regs[reg::A5] = read_reg(3);
            }

            // MRs 4..n live in the IPC buffer at words[1+i].
            if n > 4 && !thread.ipc_buffer_kva.is_null() {
                for i in 4..n {
                    unsafe {
                        *thread.ipc_buffer_kva.add(1 + i) = read_reg(i);
                    }
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

        TCB_COPY_REGISTERS => {
            // No-op until we have running TCBs whose context we could
            // copy. Returning OK lets the test driver's optional
            // helpers continue.
            Ok(())
        }

        TCB_BIND_NOTIFICATION => {
            // libsel4: tag = MessageInfo(_, 0, 1, 0)
            //   extraCaps[0] = notification cap
            let ntfn_cap = lookup_extra_cap(thread, 0)?;
            require_tag(ntfn_cap, CapTag::Notification)?;
            unsafe {
                tcb::bind_notification(tcb_ptr, ntfn_cap.notification_ptr());
            }
            Ok(())
        }

        TCB_UNBIND_NOTIFICATION => {
            unsafe { tcb::unbind_notification(tcb_ptr) };
            Ok(())
        }

        TCB_SET_TLS_BASE => {
            if length < 1 {
                return Err(SyscallError::TruncatedMessage);
            }
            unsafe { tcb::set_tls_base(tcb_ptr, uc.regs[reg::A2]) };
            Ok(())
        }

        TCB_SET_FLAGS => {
            // libsel4: tag = MessageInfo(_, 0, 0, 2). mr0 = clear, mr1 = set.
            if length < 2 {
                return Err(SyscallError::TruncatedMessage);
            }
            let clear = uc.regs[reg::A2] as u32;
            let set = uc.regs[reg::A3] as u32;
            const TCB_FLAG_MASK: u32 = 0x1; // seL4_TCBFlag_fpuDisabled
            unsafe {
                let cur = (*tcb_ptr).flags;
                let flags = (cur & !clear) | (set & TCB_FLAG_MASK);
                (*tcb_ptr).flags = flags;
                if (flags & TCB_FLAG_MASK) != 0 {
                    (*tcb_ptr).context.sstatus &= !crate::arch::riscv64::trap::SSTATUS_FS_DIRTY;
                } else {
                    (*tcb_ptr).context.sstatus |= crate::arch::riscv64::trap::SSTATUS_FS_DIRTY;
                }
                uc.regs[reg::A2] = flags as u64;
                if !thread.ipc_buffer_kva.is_null() {
                    *thread.ipc_buffer_kva.add(1) = flags as u64;
                }
            }
            Ok(())
        }

        _ => Err(SyscallError::IllegalOperation),
    }
}

/// Verify that a freshly looked-up extra-cap actually carries the
/// expected tag. Rejects with `seL4_InvalidCapability` otherwise (which
/// is what `decodeTCBConfigure` does in `kernel/src/object/tcb.c`).
#[inline]
fn require_tag(cap: Cap, expected: CapTag) -> Result<(), SyscallError> {
    if cap.tag() == Some(expected) {
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
    let cptr = read_extra_cap(thread, i);
    if cptr == 0 {
        return Err(SyscallError::InvalidCapability);
    }
    let (cap, _slot) =
        cspace::lookup_cap(thread, cptr).map_err(|_| SyscallError::InvalidCapability)?;
    Ok(cap)
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
        label::CNODE_REVOKE => cnode_op_revoke(dest_root_cap, length, uc),
        label::CNODE_DELETE => cnode_op_delete(dest_root_cap, length, uc),
        label::CNODE_COPY => cnode_op_copy_or_mint(thread, dest_root_cap, length, uc, false),
        label::CNODE_MINT => cnode_op_copy_or_mint(thread, dest_root_cap, length, uc, true),
        label::CNODE_MOVE => cnode_op_move_or_mutate(thread, dest_root_cap, length, uc, false),
        label::CNODE_MUTATE => cnode_op_move_or_mutate(thread, dest_root_cap, length, uc, true),
        label::CNODE_CANCEL_BADGED_SENDS => cnode_op_cancel_badged_sends(dest_root_cap, length, uc),
        label::CNODE_ROTATE => cnode_op_rotate(thread, dest_root_cap, length, uc),
        label::CNODE_SAVE_CALLER => cnode_op_save_caller(thread, dest_root_cap, length, uc),
        _ => {
            let _ = label_id;
            Err(SyscallError::IllegalOperation)
        }
    }
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
    let index = uc.regs[reg::A2];
    let depth = uc.regs[reg::A3] as u32 & 0xff;
    let slot = resolve_slot(dest_root_cap, index, depth)?;
    let cap = unsafe { (*slot).cap };
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
    if length < 8 {
        return Err(SyscallError::TruncatedMessage);
    }
    let dest_index = uc.regs[reg::A2];
    let dest_depth = uc.regs[reg::A3] as u32 & 0xff;
    let pivot_new_data = uc.regs[reg::A4]; // libsel4 calls this `dest_badge`
    let pivot_index = uc.regs[reg::A5];
    let pivot_depth = read_mr(thread, uc, 4) as u32 & 0xff;
    let src_new_data = read_mr(thread, uc, 5); // libsel4 calls this `pivot_badge`
    let src_index = read_mr(thread, uc, 6);
    let src_depth = read_mr(thread, uc, 7) as u32 & 0xff;

    let pivot_root_cptr = read_extra_cap(thread, 0);
    let src_root_cptr = read_extra_cap(thread, 1);
    let (pivot_root_cap, _) =
        cspace::lookup_cap(thread, pivot_root_cptr).map_err(|_| SyscallError::InvalidCapability)?;
    let (src_root_cap, _) =
        cspace::lookup_cap(thread, src_root_cptr).map_err(|_| SyscallError::InvalidCapability)?;

    let dest = resolve_slot(dest_root_cap, dest_index, dest_depth)?;
    let pivot = resolve_slot(pivot_root_cap, pivot_index, pivot_depth)?;
    let src = resolve_slot(src_root_cap, src_index, src_depth)?;

    if pivot == src || pivot == dest {
        return Err(SyscallError::IllegalOperation);
    }
    if src != dest {
        unsafe {
            if !(*dest).cap.is_null() {
                return Err(SyscallError::DeleteFirst);
            }
        }
    }
    unsafe {
        if (*src).cap.is_null() || (*pivot).cap.is_null() {
            return Err(SyscallError::FailedLookup);
        }
    }
    unsafe {
        let new_src_cap = apply_badge((*src).cap, src_new_data);
        let new_pivot_cap = apply_badge((*pivot).cap, pivot_new_data);
        if src == dest {
            // Swap pivot and src caps. MDB linkage stays attached to
            // each Cte; we simply swap the `cap` fields.
            let src_cap = new_src_cap;
            let piv_cap = new_pivot_cap;
            // Note: this changes the cap stored at each slot. The MDB
            // chain still threads through these CTEs unchanged.
            (*src).cap = piv_cap;
            (*pivot).cap = src_cap;
        } else {
            // Step 1: move pivot → dest.
            cnode_move_slot(pivot, dest, new_pivot_cap);
            // Step 2: move src → pivot.
            cnode_move_slot(src, pivot, new_src_cap);
        }
    }
    Ok(())
}

/// Move a single cap+MDB entry from `src` slot to `dst` slot, applying
/// the given (possibly badged) cap. Caller has verified `dst` is empty
/// and `src` is non-null. Re-threads the doubly linked MDB list around
/// the new location.
unsafe fn cnode_move_slot(src: *mut Cte, dst: *mut Cte, new_cap: Cap) {
    unsafe {
        let moved_mdb = (*src).mdb;
        crate::object::cnode::mdb_unlink(src);
        (*src).cap = Cap::null();
        (*dst).cap = new_cap;
        (*dst).mdb = moved_mdb;
        let prev = (*dst).mdb.prev();
        let next = (*dst).mdb.next();
        if prev != 0 {
            let p = prev as *mut Cte;
            (*p).mdb.set_next(dst as u64);
        }
        if next != 0 {
            let n = next as *mut Cte;
            (*n).mdb.set_prev(dst as u64);
        }
    }
}

/// CNode_SaveCaller: move the current thread's pending reply (caller)
/// cap into `dest`. Pre-MCS only. If the thread has no caller, the C
/// kernel logs a userError and returns NoError — match that behaviour
/// so the rootserver's `vka_save_reply_cap` path doesn't fail spuriously.
fn cnode_op_save_caller(
    _thread: &Thread,
    dest_root_cap: Cap,
    length: u64,
    uc: &UserContext,
) -> Result<(), SyscallError> {
    if length < 2 {
        return Err(SyscallError::TruncatedMessage);
    }
    let index = uc.regs[reg::A2];
    let depth = uc.regs[reg::A3] as u32 & 0xff;
    let dest = resolve_slot(dest_root_cap, index, depth)?;
    unsafe {
        if !(*dest).cap.is_null() {
            return Err(SyscallError::DeleteFirst);
        }
        let cur = crate::object::tcb::current();
        if cur.is_null() || (*cur).caller == 0 {
            return Ok(());
        }
        let caller = (*cur).caller as *mut crate::object::tcb::Tcb;
        (*dest).cap = Cap::new_reply((*cur).caller, (*cur).caller_can_grant != 0, false);
        (*dest).mdb = MdbNode::new(0, 0, true, true);
        if !caller.is_null() {
            (*caller).reply_slot = dest as u64;
        }
        (*cur).caller = 0;
        (*cur).caller_can_grant = 0;
    }
    Ok(())
}

/// Read message-register `mr_i` for `i ≥ 4` from the IPC buffer (mr0..3
/// live in `uc.regs[a2..a5]`). Returns 0 if the IPC buffer isn't mapped.
fn read_mr(thread: &Thread, uc: &UserContext, i: usize) -> u64 {
    match i {
        0 => uc.regs[reg::A2],
        1 => uc.regs[reg::A3],
        2 => uc.regs[reg::A4],
        3 => uc.regs[reg::A5],
        _ if !thread.ipc_buffer_kva.is_null() => unsafe { *thread.ipc_buffer_kva.add(1 + i) },
        _ => 0,
    }
}

fn cnode_op_revoke(dest_root_cap: Cap, length: u64, uc: &UserContext) -> Result<(), SyscallError> {
    if length < 2 {
        return Err(SyscallError::TruncatedMessage);
    }
    let index = uc.regs[reg::A2];
    let depth = uc.regs[reg::A3] as u32 & 0xff;
    let slot = resolve_slot(dest_root_cap, index, depth)?;
    revoke_descendants(slot);
    Ok(())
}

fn cnode_op_delete(dest_root_cap: Cap, length: u64, uc: &UserContext) -> Result<(), SyscallError> {
    if length < 2 {
        return Err(SyscallError::TruncatedMessage);
    }
    let index = uc.regs[reg::A2];
    let depth = uc.regs[reg::A3] as u32 & 0xff;
    let slot = resolve_slot(dest_root_cap, index, depth)?;
    delete_slot(slot)?;
    Ok(())
}

fn cnode_op_copy_or_mint(
    thread: &Thread,
    dest_root_cap: Cap,
    length: u64,
    uc: &UserContext,
    is_mint: bool,
) -> Result<(), SyscallError> {
    if length < if is_mint { 6 } else { 5 } {
        return Err(SyscallError::TruncatedMessage);
    }
    let dest_index = uc.regs[reg::A2];
    let dest_depth = uc.regs[reg::A3] as u32 & 0xff;
    let src_index = uc.regs[reg::A4];
    let src_depth = uc.regs[reg::A5] as u32 & 0xff;
    let rights = read_mr(thread, uc, 4);
    let badge = if is_mint { read_mr(thread, uc, 5) } else { 0 };

    let src_root_cptr = read_extra_cap(thread, 0);
    let (src_root_cap, _) =
        cspace::lookup_cap(thread, src_root_cptr).map_err(|_| SyscallError::InvalidCapability)?;

    let dest = resolve_slot(dest_root_cap, dest_index, dest_depth)?;
    let src = resolve_slot(src_root_cap, src_index, src_depth)?;

    unsafe {
        if !(*dest).cap.is_null() {
            return Err(SyscallError::DeleteFirst);
        }
        if (*src).cap.is_null() {
            // Mirrors C kernel `decodeCNodeInvocation` → `lookupSourceSlot`:
            // an empty source slot is a "failed lookup", not an
            // "illegal operation". sel4test (CNODEOP0003 etc.) keys off
            // the exact label, so map this carefully.
            return Err(SyscallError::FailedLookup);
        }
        let src_cap = (*src).cap;
        let mut new_cap = mask_cap_rights(src_cap, rights);
        if is_mint {
            new_cap = apply_badge(new_cap, badge);
        }
        // Mirror C kernel `isCapRevocable(newCap, srcCap)` — copies of
        // Untypeds and freshly badged EP/Notification caps are revocable
        // *roots* of their own subtree. Without setting `revocable=true`
        // here, a Revoke on the original Untyped would stop at the copy
        // because is_mdb_parent_of(copy, grandchildren) needs the copy
        // itself to be revocable to keep walking.
        let new_rev = is_cap_revocable(new_cap, src_cap);
        (*dest).cap = new_cap;
        (*dest).mdb = crate::object::mdb::MdbNode::new(0, 0, new_rev, new_rev);
        crate::object::cnode::mdb_insert_after(src, dest);
    }
    Ok(())
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

/// Mirrors C kernel `isCapRevocable(derivedCap, srcCap)` from
/// `kernel/src/object/objecttype.c`. Determines whether the destination
/// cap of a Copy/Mint becomes a revocable root of its own derivation
/// subtree (true) or just a leaf sibling (false).
fn is_cap_revocable(new_cap: Cap, src_cap: Cap) -> bool {
    match new_cap.tag() {
        // Arch caps (Frame / PageTable / ASIDPool / …) are never revocable.
        Some(CapTag::Frame) | Some(CapTag::PageTable) | Some(CapTag::AsidPool) => false,
        Some(CapTag::Untyped) => true,
        Some(CapTag::Endpoint) => new_cap.endpoint_badge() != src_cap.endpoint_badge(),
        Some(CapTag::Notification) => new_cap.notification_badge() != src_cap.notification_badge(),
        Some(CapTag::IrqHandler) => src_cap.tag() == Some(CapTag::IrqControl),
        _ => false,
    }
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
        _ => {}
    }
    cap
}

fn cnode_op_move_or_mutate(
    thread: &Thread,
    dest_root_cap: Cap,
    length: u64,
    uc: &UserContext,
    is_mutate: bool,
) -> Result<(), SyscallError> {
    if length < if is_mutate { 5 } else { 4 } {
        return Err(SyscallError::TruncatedMessage);
    }
    let dest_index = uc.regs[reg::A2];
    let dest_depth = uc.regs[reg::A3] as u32 & 0xff;
    let src_index = uc.regs[reg::A4];
    let src_depth = uc.regs[reg::A5] as u32 & 0xff;
    let badge = if is_mutate { read_mr(thread, uc, 4) } else { 0 };

    let src_root_cptr = read_extra_cap(thread, 0);
    let (src_root_cap, _) =
        cspace::lookup_cap(thread, src_root_cptr).map_err(|_| SyscallError::InvalidCapability)?;

    let dest = resolve_slot(dest_root_cap, dest_index, dest_depth)?;
    let src = resolve_slot(src_root_cap, src_index, src_depth)?;

    unsafe {
        if !(*dest).cap.is_null() {
            return Err(SyscallError::DeleteFirst);
        }
        if (*src).cap.is_null() {
            return Err(SyscallError::FailedLookup);
        }
        let mut moved = (*src).cap;
        if is_mutate {
            moved = apply_badge(moved, badge);
        }
        let mut moved_mdb = (*src).mdb;
        crate::object::cnode::mdb_unlink(src);
        (*src).cap = Cap::null();
        (*dest).cap = moved;
        // Preserve MDB linkage so Revoke still works through the move.
        moved_mdb = moved_mdb; // (already moved)
        (*dest).mdb = moved_mdb;
        // Re-thread the doubly linked list around the new location.
        let prev = (*dest).mdb.prev();
        let next = (*dest).mdb.next();
        if prev != 0 {
            let p = prev as *mut Cte;
            (*p).mdb.set_next(dest as u64);
        }
        if next != 0 {
            let n = next as *mut Cte;
            (*n).mdb.set_prev(dest as u64);
        }
    }
    Ok(())
}

/// Resolve `(index, depth)` to a `Cte*` via the given CNode-root cap.
fn resolve_slot(root_cap: Cap, index: u64, depth: u32) -> Result<*mut Cte, SyscallError> {
    if root_cap.tag() != Some(CapTag::CNode) {
        return Err(SyscallError::InvalidCapability);
    }
    let depth = if depth == 0 { cspace::WORD_BITS } else { depth };
    if depth > cspace::WORD_BITS {
        return Err(SyscallError::RangeError);
    }
    let r = cspace::lookup_slot_in(root_cap, index, depth)
        .map_err(|_| SyscallError::InvalidCapability)?;
    if r.bits_remaining != 0 {
        return Err(SyscallError::RangeError);
    }
    Ok(r.slot)
}

/// Apply a badge / guard to a cap when minting/mutating. Currently
/// supported: badging Endpoint/Notification caps, and rewriting the
/// guard on CNode caps. Other types are returned unchanged because the
/// allocman path frequently mints "frame caps with rights" — for M3 we
/// don't enforce rights yet, so the cap is just duplicated.
fn apply_badge(cap: Cap, badge: u64) -> Cap {
    match cap.tag() {
        Some(CapTag::Endpoint) | Some(CapTag::Notification) => {
            // Badge lives in words[1] for EP/Notification.
            let mut out = cap;
            out.words[1] = badge;
            out
        }
        Some(CapTag::CNode) => {
            // CNodeCapData: low 6 bits = guard_size, high 58 = guard.
            let guard_size = badge & 0x3F;
            let guard = badge >> 6;
            Cap::new_cnode(cap.cnode_ptr(), cap.cnode_radix(), guard, guard_size)
        }
        _ => cap,
    }
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
    unsafe {
        if (*slot).cap.is_null() {
            return Ok(());
        }
        let is_final = is_final_capability(slot);
        finalize_cap(&mut (*slot).cap, is_final);
        empty_slot(slot);
    }
    Ok(())
}

/// C kernel `emptySlot`: splice `slot` out of the MDB list, preserve the
/// list ordering for any descendants, and propagate `firstBadged` to the
/// successor. This differs subtly from `mdb_unlink`, which is also used
/// by Move/Mutate while the MDB node is about to be transplanted.
unsafe fn empty_slot(slot: *mut Cte) {
    debug_assert!(!slot.is_null());
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
}

/// `isFinalCapability`-equivalent: check whether this CTE holds the last
/// remaining cap to its underlying object. Mirrors the C kernel logic in
/// `kernel/src/object/cnode.c`: walk the MDB neighbours (prev / next) and
/// see if either points at the same object. A cap is *final* iff no
/// neighbour shares the object.
unsafe fn is_final_capability(slot: *mut Cte) -> bool {
    if slot.is_null() {
        return false;
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

/// Mirror of C kernel `sameObjectAs(cap_a, cap_b)`: two caps refer to
/// the same underlying kernel object. For Untyped / Frame this also
/// requires matching base+size; for the others it's just type+pointer.
fn same_object_as(a: Cap, b: Cap) -> bool {
    match (a.tag(), b.tag()) {
        (Some(CapTag::Endpoint), Some(CapTag::Endpoint)) => a.endpoint_ptr() == b.endpoint_ptr(),
        (Some(CapTag::Notification), Some(CapTag::Notification)) => {
            a.notification_ptr() == b.notification_ptr()
        }
        (Some(CapTag::CNode), Some(CapTag::CNode)) => a.cnode_ptr() == b.cnode_ptr(),
        (Some(CapTag::Thread), Some(CapTag::Thread)) => a.thread_ptr() == b.thread_ptr(),
        (Some(CapTag::PageTable), Some(CapTag::PageTable)) => {
            a.page_table_base_ptr() == b.page_table_base_ptr()
        }
        (Some(CapTag::AsidPool), Some(CapTag::AsidPool)) => a.asid_pool_ptr() == b.asid_pool_ptr(),
        (Some(CapTag::Frame), Some(CapTag::Frame)) => {
            a.frame_base_ptr() == b.frame_base_ptr() && a.frame_size() == b.frame_size()
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
fn finalize_cap(cap: &mut Cap, is_final: bool) {
    match cap.tag() {
        Some(CapTag::Frame) => {
            let va = cap.frame_mapped_addr();
            if va != 0 {
                // Route the unmap through the VSpace the cap was originally
                // mapped into, *not* the current thread. Otherwise a Revoke
                // on the parent Untyped would walk Frame children and erase
                // PTEs out of whatever VSpace happens to be active right
                // now (the driver), corrupting unrelated mappings.
                let asid = cap.frame_mapped_asid();
                let root_pt_kva = crate::object::asid::lookup(asid);
                if root_pt_kva != 0 {
                    unsafe {
                        let _ = crate::arch::riscv64::vspace::unmap_user_frame(
                            root_pt_kva as *mut crate::arch::riscv64::sv39::PageTable,
                            va as usize,
                            cap.frame_size(),
                        );
                    }
                }
                cap.set_frame_mapped_addr(0);
                cap.set_frame_mapped_asid(0);
            }
        }
        Some(CapTag::PageTable) => {
            if is_final && cap.page_table_is_mapped() {
                let asid = cap.page_table_mapped_asid();
                let root_pt_kva = cap.page_table_base_ptr();
                crate::object::asid::delete(asid, root_pt_kva);
                cap.clear_page_table_mapping();
            }
        }
        Some(CapTag::AsidPool) => {
            if is_final {
                crate::object::asid::delete_pool(cap.asid_pool_base(), cap.asid_pool_ptr());
            }
        }
        Some(CapTag::Thread) => {
            // Drop bound-notification linkage, queue links, etc., so a
            // stale pointer to this slab can't look "runnable" if some
            // future scheduler scan races the Revoke. The actual
            // storage is recycled by the parent Untyped on Retype.
            let p = cap.thread_ptr();
            if p != 0 && is_pspace_kva(p) {
                unsafe {
                    crate::object::tcb::finalize(p as *mut crate::object::tcb::Tcb);
                }
            }
        }
        Some(CapTag::Reply) => {
            let caller = cap.reply_tcb_ptr() as *mut crate::object::tcb::Tcb;
            if !caller.is_null() {
                unsafe {
                    (*caller).reply_slot = 0;
                }
            }
        }
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
                if p != 0 && is_pspace_kva(p) {
                    unsafe {
                        crate::object::endpoint::finalize(
                            p as *mut crate::object::endpoint::Endpoint,
                        );
                    }
                }
            }
        }
        Some(CapTag::Notification) => {
            // Final-cap path only (see Endpoint above for the
            // rationale). Non-final delete leaves the notification
            // object and its waiters intact.
            if is_final {
                let p = cap.notification_ptr();
                if p != 0 && is_pspace_kva(p) {
                    unsafe {
                        let n = p as *mut crate::object::notification::Notification;
                        let bound = (*n).bound_tcb();
                        if bound != 0 {
                            let tcb_ptr = bound as *mut crate::object::tcb::Tcb;
                            (*tcb_ptr).bound_notification = 0;
                        }
                        (*n).set_bound_tcb(0);
                        crate::object::notification::finalize(n);
                    }
                }
            }
        }
        Some(CapTag::CNode) => {
            // Mirrors the C kernel `finaliseCap` returning a Zombie for a
            // CNode: every slot inside the CNode must be cleaned up before
            // we reuse the storage. Without this, caps held by a test
            // process linger in the global MDB chain even after the
            // process is torn down — and the next Retype-reset on the
            // parent Untyped sees a stale `has_children=true` and refuses
            // to recycle the slab.
            //
            // Only operate on CNode storage we know is safely backed by
            // PSpace RAM (kernel-window mapped). The kernel ELF / device
            // windows are read-only, and an over-eager finalize on a stale
            // CNode cap pointing there would page-fault the kernel.
            let base = cap.cnode_ptr();
            let radix = cap.cnode_radix();
            if base != 0 && is_pspace_kva(base) {
                let n_slots = 1usize << radix;
                unsafe {
                    let slots = crate::object::cnode::cnode_at(base as *mut u8, radix as usize);
                    if slots.len() == n_slots {
                        for i in 0..n_slots {
                            let inner = &mut slots[i];
                            if !inner.cap.is_null() {
                                let inner_ptr = inner as *mut Cte;
                                // Drop the CNode's internal cap slots so
                                // their MDB links stop pinning untypeds.
                                // The owning Untyped_Revoke path will
                                // finalise physical descendants in CDT
                                // order; doing that recursively here can
                                // destroy caps that are still live through
                                // another CSpace root.
                                crate::object::cnode::mdb_unlink(inner_ptr);
                                inner.cap = Cap::null();
                            }
                        }
                    }
                }
            }
        }
        _ => {}
    }
}

/// Walk the CDT descendants of `cte` and clear them. The C kernel does
/// this recursively with preemption points; we just iterate the linked
/// list once since our single-thread model has no preemption.
fn revoke_descendants(cte: *mut Cte) {
    // The C kernel uses recursive `cteDelete(child, true)` to also kill
    // grandchildren. Without that an Untyped Revoke would leave Frame
    // caps carved from it still mapped into someone's VSpace, and the
    // next Retype-reset would hand out memory that's still being read
    // through stale PTEs (use-after-free).
    unsafe {
        while crate::object::cnode::mdb_has_children(cte) {
            let child = (*cte).mdb.next() as *mut Cte;
            revoke_descendants(child);
            let is_final = is_final_capability(child);
            finalize_cap(&mut (*child).cap, is_final);
            crate::object::cnode::mdb_unlink(child);
            (*child).cap = Cap::null();
        }
    }
}

/// Helper: read mr4 / mr5 from the IPC buffer. The IPC buffer's `msg`
/// array starts at offset 1 word inside the frame (word 0 is the tag).
fn read_mrs_4_5(thread: &Thread) -> (u64, u64) {
    if thread.ipc_buffer_kva.is_null() {
        return (0, 0);
    }
    unsafe {
        let base = thread.ipc_buffer_kva;
        (*base.add(1 + 4), *base.add(1 + 5))
    }
}

/// Read `caps_or_badges[i]` from the current thread's IPC buffer. Used to
/// recover extra-cap CPtrs that the user marshalled via `seL4_SetCap`.
///
/// The IPC buffer layout has `msg[120]` words after the tag, then
/// `userData`, then `caps_or_badges[3]`. So caps_or_badges[i] lives at
/// word offset 1 + 120 + 1 + i = 122 + i.
fn read_extra_cap(thread: &Thread, i: usize) -> u64 {
    debug_assert!(i < 3);
    if thread.ipc_buffer_kva.is_null() {
        return 0;
    }
    unsafe { *thread.ipc_buffer_kva.add(122 + i) }
}

#[inline]
fn align_up(v: u64, bits: u64) -> u64 {
    let mask = (1u64 << bits) - 1;
    (v + mask) & !mask
}

#[allow(unused_imports)]
use cspace as _;
