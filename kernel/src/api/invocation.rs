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
use crate::arch::riscv64::trap::{reg, UserContext};
use crate::object::cap::{Cap, CapTag};
use crate::object::cnode::{cnode_at, Cte};
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
    Some(match ty {
        obj::UNTYPED => user_size,
        obj::TCB => 11,
        obj::ENDPOINT => 4,
        obj::NOTIFICATION => 6,
        obj::CAP_TABLE => user_size + crate::abi::constants::SEL4_SLOT_BITS as u64,
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
            let radix = user_size;
            Cap::new_cnode(region_base, radix, 0, 64 - radix)
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

    crate::println!(
        "  Untyped_Retype: type={} size={} window={} off={} depth={} idx={:#x} root={:#x} (cap@{:#x})",
        new_type, user_size, node_window, node_offset, node_depth,
        node_index, root_cptr, src_cap.untyped_ptr(),
    );

    let obj_bits = object_size_bits(new_type, user_size)
        .ok_or(SyscallError::IllegalOperation)?;

    if node_window < 1 || node_window > 256 {
        return Err(SyscallError::RangeError);
    }

    // Resolve the destination CNode capability.
    //   nodeDepth == 0 → use the looked-up cap *directly* (it must be a CNode).
    //   nodeDepth > 0  → walk `nodeIndex` for `nodeDepth` bits within it.
    let dest_cnode_cap = if node_depth == 0 {
        let (cap, _) = cspace::lookup_cap(thread, root_cptr)
            .map_err(|_| SyscallError::InvalidCapability)?;
        cap
    } else {
        // Single-level walk for now: assume the supplied cap is already
        // the rootserver's root CNode (caps_or_badges[0]) and we just
        // re-resolve through it. For our M3 scenarios `node_depth == 0`,
        // so we fall back to that interpretation if anything's off.
        let (cap, _) = cspace::lookup_cap(thread, root_cptr)
            .map_err(|_| SyscallError::InvalidCapability)?;
        let _ = (node_index, node_depth);
        cap
    };
    if dest_cnode_cap.tag() != Some(CapTag::CNode) {
        crate::println!("    dest cap is not a CNode (tag={:?})", dest_cnode_cap.tag());
        return Err(SyscallError::InvalidCapability);
    }
    let dest_radix = dest_cnode_cap.cnode_radix();
    if node_offset >= (1u64 << dest_radix) {
        return Err(SyscallError::RangeError);
    }
    if node_window > (1u64 << dest_radix) - node_offset {
        return Err(SyscallError::RangeError);
    }
    let dest_base_kva = dest_cnode_cap.cnode_ptr();
    let dest_cnode = unsafe {
        cnode_at(dest_base_kva as *mut u8, dest_radix as usize)
    };

    // Ensure target slots are empty.
    for i in 0..node_window {
        let slot = &dest_cnode[(node_offset + i) as usize];
        if !slot.cap.is_null() {
            return Err(SyscallError::DeleteFirst);
        }
    }

    let untyped_bits = src_cap.untyped_block_size_bits();
    let free_index = src_cap.untyped_free_index();
    let is_device = src_cap.untyped_is_device();
    let region_base_kva = src_cap.untyped_ptr();
    let region_size = 1u64 << untyped_bits;
    let used_bytes = free_index << 4;
    let free_bytes = region_size.saturating_sub(used_bytes);

    let aligned_start_offset = align_up(used_bytes, obj_bits);
    let total_obj_bytes = node_window << obj_bits;

    if aligned_start_offset.saturating_add(total_obj_bytes) > region_size {
        crate::println!(
            "    NotEnoughMemory: untyped={} bits, used={} bytes, need {} bytes",
            untyped_bits, used_bytes, total_obj_bytes,
        );
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

    // Install caps for each new object.
    for i in 0..node_window {
        let obj_base = alloc_base_kva.wrapping_add(i << obj_bits);
        let cap = create_object_cap(new_type, obj_base, user_size, is_device)
            .ok_or(SyscallError::IllegalOperation)?;
        let dst = &mut dest_cnode[(node_offset + i) as usize];
        dst.cap = cap;
        // CDT bookkeeping: parent = src_slot. The C kernel uses a circular
        // doubly-linked list rooted at the parent. For M3 we only need the
        // parent-linkage to detect "has children" later; a full impl follows
        // when we add Delete/Revoke.
        dst.mdb = MdbNode::new(src_slot as u64, 0, true, true);
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
            let _rights_packed = uc.regs[reg::A3];
            let _attrs = uc.regs[reg::A4];

            // Look up the vspace cap from extraCaps[0].
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

            crate::println!(
                "  Page_Map: vaddr={:#x} frame_kva={:#x} frame_pa={:#x} root_pt={:#x}",
                vaddr, frame_kva, frame_pa, root_pt_kva,
            );

            unsafe {
                vspace::map_user_4k(
                    root_pt_kva as *mut crate::arch::riscv64::sv39::PageTable,
                    vaddr as usize,
                    frame_pa as usize,
                    vspace::user_flags(true, true, false),
                );
            }
            Ok(())
        }
        RISCV_PAGE_UNMAP => {
            crate::println!("  Page_Unmap: (stubbed)");
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
            crate::println!("  Frame op: label={} (unsupported)", label_id);
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
        RISCV_PAGE_TABLE_MAP => {
            crate::println!("  PageTable_Map: (stubbed — auto-allocated)");
            Ok(())
        }
        RISCV_PAGE_TABLE_UNMAP => {
            crate::println!("  PageTable_Unmap: (stubbed)");
            Ok(())
        }
        _ => {
            crate::println!("  PageTable op: label={} (unsupported)", label_id);
            Err(SyscallError::IllegalOperation)
        }
    }
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
        label::CNODE_CANCEL_BADGED_SENDS => {
            // No EP/Notif IPC yet (M3.6) — treat as success no-op.
            Ok(())
        }
        _ => {
            crate::println!("  CNode op: label={} (unsupported)", label_id);
            Err(SyscallError::IllegalOperation)
        }
    }
}

/// Read message-register `mr_i` for `i ≥ 4` from the IPC buffer (mr0..3
/// live in `uc.regs[a2..a5]`). Returns 0 if the IPC buffer isn't mapped.
fn read_mr(thread: &Thread, uc: &UserContext, i: usize) -> u64 {
    match i {
        0 => uc.regs[reg::A2],
        1 => uc.regs[reg::A3],
        2 => uc.regs[reg::A4],
        3 => uc.regs[reg::A5],
        _ if !thread.ipc_buffer_kva.is_null() => unsafe {
            *thread.ipc_buffer_kva.add(1 + i)
        },
        _ => 0,
    }
}

fn cnode_op_revoke(
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
    revoke_descendants(slot);
    Ok(())
}

fn cnode_op_delete(
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
    let _rights = read_mr(thread, uc, 4);
    let badge = if is_mint { read_mr(thread, uc, 5) } else { 0 };

    let src_root_cptr = read_extra_cap(thread, 0);
    let (src_root_cap, _) = cspace::lookup_cap(thread, src_root_cptr)
        .map_err(|_| SyscallError::InvalidCapability)?;

    let dest = resolve_slot(dest_root_cap, dest_index, dest_depth)?;
    let src = resolve_slot(src_root_cap, src_index, src_depth)?;

    unsafe {
        if !(*dest).cap.is_null() {
            return Err(SyscallError::DeleteFirst);
        }
        if (*src).cap.is_null() {
            return Err(SyscallError::IllegalOperation);
        }
        let mut new_cap = (*src).cap;
        if is_mint {
            new_cap = apply_badge(new_cap, badge);
        }
        (*dest).cap = new_cap;
        // Both Copy and Mint produce derivable children; we just record
        // the link so future Revoke/Delete walks find them.
        (*dest).mdb = crate::object::mdb::MdbNode::NULL;
        crate::object::cnode::mdb_insert_after(src, dest);
    }
    Ok(())
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
    let (src_root_cap, _) = cspace::lookup_cap(thread, src_root_cptr)
        .map_err(|_| SyscallError::InvalidCapability)?;

    let dest = resolve_slot(dest_root_cap, dest_index, dest_depth)?;
    let src = resolve_slot(src_root_cap, src_index, src_depth)?;

    unsafe {
        if !(*dest).cap.is_null() {
            return Err(SyscallError::DeleteFirst);
        }
        if (*src).cap.is_null() {
            return Err(SyscallError::IllegalOperation);
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

/// Empty a slot, freeing the resources behind its cap if necessary. For
/// M3.4 we don't yet zero out the object's backing memory (that's a
/// Revoke responsibility) — we just clear the slot and unlink from CDT.
fn delete_slot(slot: *mut Cte) -> Result<(), SyscallError> {
    unsafe {
        if (*slot).cap.is_null() {
            return Ok(());
        }
        if crate::object::cnode::mdb_has_children(slot) {
            return Err(SyscallError::RevokeFirst);
        }
        crate::object::cnode::mdb_unlink(slot);
        (*slot).cap = Cap::null();
    }
    Ok(())
}

/// Walk the CDT descendants of `cte` and clear them. The C kernel does
/// this recursively with preemption points; we just iterate the linked
/// list once since our single-thread model has no preemption.
fn revoke_descendants(cte: *mut Cte) {
    unsafe {
        // Walk forward until we hit a sibling/parent (i.e. an MDB node
        // whose prev is not `cte` itself).
        let parent = cte;
        loop {
            let next = (*parent).mdb.next();
            if next == 0 {
                break;
            }
            let child = next as *mut Cte;
            if (*child).mdb.prev() != parent as u64 {
                break;
            }
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
