use core::cmp::min;
use core::ptr;

use crate::allocator::Allocator;
use crate::consts::*;
use crate::sel4::{
    call_checked, cap_rights, cnode_cap_data, msg_info, msg_label, read_ipc_mr, sel4_call,
};
use crate::types::{Child, Mapping};
use crate::util::*;

static mut MAPPINGS: [Mapping; MAX_MAPPINGS] = [Mapping {
    proc_slot: 0,
    pid: 0,
    child_page: 0,
    alias_page: 0,
    frame_slot: 0,
    alias_slot: 0,
    writable: false,
    executable: false,
}; MAX_MAPPINGS];
static mut MAPPING_COUNT: usize = 0;

#[derive(Copy, Clone)]
struct FramePoolEntry {
    proc_slot: usize,
    frame_slot: u64,
}

static mut FRAME_POOL: [FramePoolEntry; FRAME_POOL_CAP] = [FramePoolEntry {
    proc_slot: 0,
    frame_slot: 0,
}; FRAME_POOL_CAP];
static mut FRAME_POOL_LEN: usize = 0;

pub(crate) fn create_child(
    alloc: &mut Allocator,
    proc_slot: usize,
    pid: u64,
    parent_pid: u64,
    fault_ep: u64,
) -> Child {
    let untyped = alloc.process_untyped(proc_slot);
    let tcb = alloc.retype_one_from(untyped, OBJ_TCB, 0);
    let cnode = alloc.retype_one_from(untyped, OBJ_CAP_TABLE, CHILD_CNODE_BITS);
    let vspace = alloc.retype_one_from(untyped, OBJ_PAGE_TABLE, 0);
    let ipc_frame = alloc.retype_one_from(untyped, OBJ_4K, 0);

    call_checked(INIT_ASID_POOL, LABEL_RISCV_ASID_POOL_ASSIGN, &[vspace], &[]);
    map_existing_frame(
        alloc,
        proc_slot,
        pid,
        ipc_frame,
        vspace,
        CHILD_IPC_BUFFER,
        true,
        false,
        false,
        false,
    );

    let mrs = [
        CHILD_FAULT_EP,
        CHILD_CNODE_BITS,
        fault_ep,
        ROOT_CNODE_DEPTH,
        cap_rights(true, true, true, true),
        pid,
    ];
    call_checked(cnode, LABEL_CNODE_MINT, &[ROOT_CNODE], &mrs);

    let cspace_data = cnode_cap_data(0, WORD_BITS - CHILD_CNODE_BITS);
    let mrs = [CHILD_FAULT_EP, cspace_data, 0, CHILD_IPC_BUFFER];
    call_checked(tcb, LABEL_TCB_CONFIGURE, &[cnode, vspace, ipc_frame], &mrs);
    call_checked(tcb, LABEL_TCB_SET_PRIORITY, &[INIT_TCB], &[254]);

    Child {
        pid,
        proc_slot,
        parent_pid,
        state: PROC_RUNNABLE,
        exit_status: 0,
        reparented_to_init: false,
        tcb,
        cnode,
        vspace,
        untyped,
        fault_ep,
        entry: 0,
        brk: 0,
        heap_start: 0,
        heap_mapped_end: 0,
        fds: [crate::types::FdEntry::closed(); MAX_FD],
        cwd: FS_ROOT_NODE,
        wait_status_ptr: 0,
        wait_reply_slot: 0,
        wait_reply_mrs: [0; 11],
        pipe_reply_slot: 0,
        pipe_reply_mrs: [0; 11],
        pipe_fd: 0,
        pipe_buf: 0,
        pipe_len: 0,
        pipe_done: 0,
    }
}

pub(crate) fn destroy_child_objects(alloc: &mut Allocator, child: &Child) {
    if child.pid == 0 {
        return;
    }
    clear_process_frame_pool(alloc, child.proc_slot);
    if child.tcb != 0 {
        call_checked(child.tcb, LABEL_TCB_SUSPEND, &[], &[]);
    }
    alloc.delete_cap_slot(child.tcb);
    alloc.delete_cap_slot(child.cnode);
    alloc.delete_cap_slot(child.vspace);
    alloc.revoke_cap_slot(child.untyped);
}

pub(crate) fn load_payload(alloc: &mut Allocator, child: &mut Child) {
    load_elf(alloc, child, PAYLOAD_ELF);
}

pub(crate) fn load_elf(alloc: &mut Allocator, child: &mut Child, elf: &[u8]) {
    if elf.len() < 64 || &elf[0..4] != b"\x7fELF" || elf[4] != 2 || elf[5] != 1 {
        log("xv6-host: bad payload ELF\n");
        halt_loop();
    }
    let entry = read_u64(elf, 24);
    let phoff = read_u64(elf, 32) as usize;
    let phentsize = read_u16(elf, 54) as usize;
    let phnum = read_u16(elf, 56) as usize;
    let mut image_end = 0u64;

    for i in 0..phnum {
        let off = phoff + i * phentsize;
        if off + 56 > elf.len() {
            log("xv6-host: truncated program headers\n");
            halt_loop();
        }
        let p_type = read_u32(elf, off);
        if p_type != 1 {
            continue;
        }
        let p_flags = read_u32(elf, off + 4);
        let p_offset = read_u64(elf, off + 8) as usize;
        let p_vaddr = read_u64(elf, off + 16);
        let p_filesz = read_u64(elf, off + 32) as usize;
        let p_memsz = read_u64(elf, off + 40);
        let writable = (p_flags & 0x2) != 0;
        let executable = (p_flags & 0x1) != 0;
        if p_offset.saturating_add(p_filesz) > elf.len() {
            log("xv6-host: segment outside payload\n");
            halt_loop();
        }

        let start = align_down(p_vaddr);
        let end = align_up(p_vaddr.saturating_add(p_memsz));
        let mut page = start;
        while page < end {
            map_fresh_child_page(alloc, child, page, writable, executable);
            page += PAGE_SIZE;
        }
        if p_filesz > 0
            && !copy_to_child_raw(alloc, child, p_vaddr, &elf[p_offset..p_offset + p_filesz])
        {
            log("xv6-host: failed to copy payload\n");
            halt_loop();
        }
        image_end = image_end.max(p_vaddr.saturating_add(p_memsz));
    }

    child.entry = entry;
    child.brk = align_up(image_end);
    child.heap_start = child.brk;
    child.heap_mapped_end = child.brk;
    log("xv6-host: payload entry=");
    print_hex(entry);
    log(" brk=");
    print_hex(child.brk);
    log("\n");
}

pub(crate) fn reset_process_mappings(alloc: &mut Allocator, pid: u64) {
    unsafe {
        let mut i = 0;
        while i < MAPPING_COUNT {
            if MAPPINGS[i].pid == pid && MAPPINGS[i].child_page != CHILD_IPC_BUFFER {
                unmap_mapping_at(alloc, i);
            }
            i += 1;
        }
    }
}

pub(crate) fn clear_process_mappings(alloc: &mut Allocator, pid: u64) {
    unsafe {
        let mut i = 0;
        while i < MAPPING_COUNT {
            if MAPPINGS[i].pid == pid {
                unmap_mapping_at(alloc, i);
            }
            i += 1;
        }
    }
}

pub(crate) fn unmap_child_range(alloc: &mut Allocator, pid: u64, start: u64, end: u64) {
    let start = align_down(start);
    let end = align_up(end);
    unsafe {
        let mut i = 0;
        while i < MAPPING_COUNT {
            let m = MAPPINGS[i];
            if m.pid == pid && m.child_page >= start && m.child_page < end {
                unmap_mapping_at(alloc, i);
            }
            i += 1;
        }
    }
}

pub(crate) fn mapping_slots_available() -> usize {
    unsafe {
        let mut free = MAX_MAPPINGS.saturating_sub(MAPPING_COUNT);
        let mut i = 0;
        while i < MAPPING_COUNT {
            if MAPPINGS[i].pid == 0 {
                free += 1;
            }
            i += 1;
        }
        free
    }
}

pub(crate) fn frame_pool_available() -> usize {
    unsafe { FRAME_POOL_LEN }
}

pub(crate) fn map_stack(alloc: &mut Allocator, child: &Child) {
    for i in 0..CHILD_STACK_PAGES {
        let va = CHILD_STACK_TOP - ((i as u64 + 1) * PAGE_SIZE);
        map_fresh_child_page(alloc, child, va, true, false);
    }
}

pub(crate) fn start_child(child: &Child) {
    let mut ctx = [0u64; USER_CONTEXT_WORDS];
    ctx[0] = child.entry;
    ctx[2] = CHILD_STACK_TOP;
    ctx[16] = 0;
    ctx[17] = 0;
    write_user_context(child.tcb, &ctx, true);
}

pub(crate) fn map_fresh_child_page(
    alloc: &mut Allocator,
    child: &Child,
    child_va: u64,
    writable: bool,
    executable: bool,
) -> u64 {
    let page = align_down(child_va);
    if let Some(idx) = lookup_mapping_index(child.pid, page) {
        let alias = ensure_alias_for_index(alloc, idx);
        return alias;
    }
    let (frame_slot, owner_proc_slot, needs_zero) = alloc_process_frame(alloc, child);
    map_existing_frame(
        alloc,
        owner_proc_slot,
        child.pid,
        frame_slot,
        child.vspace,
        page,
        writable,
        executable,
        true,
        needs_zero,
    )
}

pub(crate) fn map_lazy_child_page(
    alloc: &mut Allocator,
    child: &Child,
    child_va: u64,
    writable: bool,
    executable: bool,
) -> u64 {
    let page = align_down(child_va);
    if let Some(idx) = lookup_mapping_index(child.pid, page) {
        return unsafe { MAPPINGS[idx].alias_page };
    }
    let (frame_slot, owner_proc_slot, needs_zero) = alloc_process_frame(alloc, child);
    map_existing_frame(
        alloc,
        owner_proc_slot,
        child.pid,
        frame_slot,
        child.vspace,
        page,
        writable,
        executable,
        false,
        needs_zero,
    )
}

fn map_existing_frame(
    alloc: &mut Allocator,
    proc_slot: usize,
    pid: u64,
    frame_slot: u64,
    vspace: u64,
    child_va: u64,
    writable: bool,
    executable: bool,
    with_alias: bool,
    zero_frame: bool,
) -> u64 {
    let (mapping_slot, alias_va) =
        register_mapping(proc_slot, pid, child_va, frame_slot, writable, executable);
    if with_alias {
        ensure_alias_for_index(alloc, mapping_slot);
        if zero_frame {
            zero_page(alias_va);
        }
    } else if zero_frame {
        zero_frame_with_temporary_alias(alloc, frame_slot, alias_va);
    }
    page_map(frame_slot, vspace, child_va, writable, executable);
    alias_va
}

fn page_map(frame_slot: u64, vspace: u64, va: u64, writable: bool, executable: bool) {
    let rights = cap_rights(false, false, true, writable);
    let attrs = if executable { 0 } else { 1 };
    call_checked(
        frame_slot,
        LABEL_RISCV_PAGE_MAP,
        &[vspace],
        &[va, rights, attrs],
    );
}

fn register_mapping(
    proc_slot: usize,
    pid: u64,
    child_page: u64,
    frame_slot: u64,
    writable: bool,
    executable: bool,
) -> (usize, u64) {
    unsafe {
        let mut slot = MAPPING_COUNT;
        let mut i = 0;
        while i < MAPPING_COUNT {
            if MAPPINGS[i].pid == 0 {
                slot = i;
                break;
            }
            i += 1;
        }
        if slot >= MAX_MAPPINGS {
            log("xv6-host: mapping table full\n");
            halt_loop();
        }
        let alias = if slot == MAPPING_COUNT {
            let alias = HOST_ALIAS_BASE + (MAPPING_COUNT as u64) * PAGE_SIZE;
            MAPPING_COUNT += 1;
            alias
        } else {
            MAPPINGS[slot].alias_page
        };
        MAPPINGS[slot] = Mapping {
            proc_slot,
            pid,
            child_page: align_down(child_page),
            alias_page: alias,
            frame_slot,
            alias_slot: 0,
            writable,
            executable,
        };
        (slot, alias)
    }
}

fn unmap_mapping_at(alloc: &mut Allocator, slot: usize) {
    unsafe {
        let m = MAPPINGS[slot];
        if m.pid == 0 {
            return;
        }
        if m.alias_slot != 0 {
            page_unmap(m.alias_slot);
        }
        if m.frame_slot != 0 {
            page_unmap(m.frame_slot);
        }
        alloc.delete_cap_slot(m.alias_slot);
        release_process_frame(alloc, m.proc_slot, m.frame_slot);
        MAPPINGS[slot] = Mapping {
            proc_slot: 0,
            pid: 0,
            child_page: 0,
            alias_page: m.alias_page,
            frame_slot: 0,
            alias_slot: 0,
            writable: false,
            executable: false,
        };
    }
}

fn page_unmap(frame_slot: u64) {
    call_checked(frame_slot, LABEL_RISCV_PAGE_UNMAP, &[], &[]);
}

pub(crate) fn is_child_page_mapped(child: &Child, va: u64) -> bool {
    lookup_mapping(child.pid, va).is_some()
}

fn lookup_mapping_index(pid: u64, child_page: u64) -> Option<usize> {
    unsafe {
        let page = align_down(child_page);
        let mut i = 0;
        while i < MAPPING_COUNT {
            let m = MAPPINGS[i];
            if m.pid == pid && m.child_page == page {
                return Some(i);
            }
            i += 1;
        }
        None
    }
}

fn lookup_mapping(pid: u64, child_page: u64) -> Option<Mapping> {
    unsafe {
        let page = align_down(child_page);
        let mut i = 0;
        while i < MAPPING_COUNT {
            let m = MAPPINGS[i];
            if m.pid == pid && m.child_page == page {
                return Some(m);
            }
            i += 1;
        }
        None
    }
}

fn alloc_process_frame(alloc: &mut Allocator, child: &Child) -> (u64, usize, bool) {
    unsafe {
        let mut i = FRAME_POOL_LEN;
        if i > 0 {
            i -= 1;
            let owner_proc_slot = FRAME_POOL[i].proc_slot;
            let frame_slot = FRAME_POOL[i].frame_slot;
            FRAME_POOL_LEN -= 1;
            FRAME_POOL[i] = FRAME_POOL[FRAME_POOL_LEN];
            FRAME_POOL[FRAME_POOL_LEN] = FramePoolEntry {
                proc_slot: 0,
                frame_slot: 0,
            };
            return (frame_slot, owner_proc_slot, true);
        }
    }
    (
        alloc.retype_one_from(child.untyped, OBJ_4K, 0),
        child.proc_slot,
        false,
    )
}

fn release_process_frame(alloc: &mut Allocator, proc_slot: usize, frame_slot: u64) {
    if frame_slot == 0 {
        return;
    }
    unsafe {
        let len = FRAME_POOL_LEN;
        if len >= FRAME_POOL_CAP {
            alloc.delete_cap_slot(frame_slot);
            return;
        }
        FRAME_POOL[len] = FramePoolEntry {
            proc_slot,
            frame_slot,
        };
        FRAME_POOL_LEN = len + 1;
    }
}

fn clear_process_frame_pool(alloc: &mut Allocator, proc_slot: usize) {
    unsafe {
        let mut i = 0;
        while i < FRAME_POOL_LEN {
            if FRAME_POOL[i].proc_slot == proc_slot {
                alloc.delete_cap_slot(FRAME_POOL[i].frame_slot);
                FRAME_POOL_LEN -= 1;
                FRAME_POOL[i] = FRAME_POOL[FRAME_POOL_LEN];
                FRAME_POOL[FRAME_POOL_LEN] = FramePoolEntry {
                    proc_slot: 0,
                    frame_slot: 0,
                };
            } else {
                i += 1;
            }
        }
    }
}

fn child_ptr(
    alloc: &mut Allocator,
    child: &Child,
    va: u64,
    write: bool,
    allow_lazy: bool,
) -> Option<*mut u8> {
    let page = align_down(va);
    let off = va - page;
    let idx = match lookup_mapping_index(child.pid, page) {
        Some(idx) => idx,
        None if allow_lazy && is_lazy_heap_addr(child, va) => {
            map_lazy_child_page(alloc, child, page, true, false);
            lookup_mapping_index(child.pid, page)?
        }
        None => return None,
    };
    let mapping = unsafe { MAPPINGS[idx] };
    if write && !mapping.writable {
        return None;
    }
    let alias_page = ensure_alias_for_index(alloc, idx);
    Some((alias_page + off) as *mut u8)
}

pub(crate) fn copy_from_child(
    alloc: &mut Allocator,
    child: &Child,
    va: u64,
    out: &mut [u8],
) -> bool {
    let mut done = 0usize;
    while done < out.len() {
        let cur = va + done as u64;
        let page_left = (PAGE_SIZE - (cur & (PAGE_SIZE - 1))) as usize;
        let n = min(page_left, out.len() - done);
        let Some(src) = child_ptr(alloc, child, cur, false, true) else {
            return false;
        };
        unsafe { ptr::copy_nonoverlapping(src as *const u8, out[done..].as_mut_ptr(), n) };
        done += n;
    }
    true
}

pub(crate) fn copy_to_child(alloc: &mut Allocator, child: &Child, va: u64, src: &[u8]) -> bool {
    let mut done = 0usize;
    while done < src.len() {
        let cur = va + done as u64;
        let page_left = (PAGE_SIZE - (cur & (PAGE_SIZE - 1))) as usize;
        let n = min(page_left, src.len() - done);
        let Some(dst) = child_ptr(alloc, child, cur, true, true) else {
            return false;
        };
        unsafe { ptr::copy_nonoverlapping(src[done..].as_ptr(), dst, n) };
        done += n;
    }
    true
}

pub(crate) fn copy_to_child_raw(alloc: &mut Allocator, child: &Child, va: u64, src: &[u8]) -> bool {
    let mut done = 0usize;
    while done < src.len() {
        let cur = va + done as u64;
        let page_left = (PAGE_SIZE - (cur & (PAGE_SIZE - 1))) as usize;
        let n = min(page_left, src.len() - done);
        let Some(dst) = child_ptr(alloc, child, cur, false, false) else {
            return false;
        };
        unsafe { ptr::copy_nonoverlapping(src[done..].as_ptr(), dst, n) };
        done += n;
    }
    true
}

pub(crate) fn copy_cstr_from_child(
    alloc: &mut Allocator,
    child: &Child,
    va: u64,
    out: &mut [u8],
) -> Option<usize> {
    for i in 0..out.len() {
        let mut b = [0u8; 1];
        if !copy_from_child(alloc, child, va + i as u64, &mut b) {
            return None;
        }
        out[i] = b[0];
        if b[0] == 0 {
            return Some(i);
        }
    }
    None
}

pub(crate) fn clone_address_space(alloc: &mut Allocator, parent: &Child, child: &Child) {
    unsafe {
        let mut i = 0;
        let live_heap_end = align_up(parent.brk);
        while i < MAPPING_COUNT {
            let m = MAPPINGS[i];
            let freed_heap_page = m.child_page >= live_heap_end && m.child_page < CHILD_HEAP_LIMIT;
            if m.pid == parent.pid && m.child_page != CHILD_IPC_BUFFER && !freed_heap_page {
                let src_alias = ensure_alias_for_index(alloc, i);
                let dst_alias =
                    map_fresh_child_page(alloc, child, m.child_page, m.writable, m.executable);
                ptr::copy_nonoverlapping(
                    src_alias as *const u8,
                    dst_alias as *mut u8,
                    PAGE_SIZE as usize,
                );
            }
            i += 1;
        }
    }
}

pub(crate) const USER_CONTEXT_WORDS: usize = 32;

pub(crate) fn read_user_context(tcb: u64) -> [u64; USER_CONTEXT_WORDS] {
    let reply = unsafe {
        sel4_call(
            tcb,
            msg_info(LABEL_TCB_READ_REGISTERS, 0, 0, 2),
            &[0, USER_CONTEXT_WORDS as u64],
        )
    };
    let err = msg_label(reply.info);
    if err != 0 {
        log("xv6-host: TCB_ReadRegisters failed err=");
        print_u64(err);
        log("\n");
        halt_loop();
    }
    let mut ctx = [0u64; USER_CONTEXT_WORDS];
    let mut i = 0;
    while i < USER_CONTEXT_WORDS {
        ctx[i] = if i < 4 {
            reply.mrs[i]
        } else {
            unsafe { read_ipc_mr(i) }
        };
        i += 1;
    }
    ctx
}

pub(crate) fn write_user_context(tcb: u64, ctx: &[u64; USER_CONTEXT_WORDS], resume: bool) {
    let mut mrs = [0u64; USER_CONTEXT_WORDS + 2];
    mrs[0] = resume as u64;
    mrs[1] = USER_CONTEXT_WORDS as u64;
    let mut i = 0;
    while i < USER_CONTEXT_WORDS {
        mrs[i + 2] = ctx[i];
        i += 1;
    }
    call_checked(tcb, LABEL_TCB_WRITE_REGISTERS, &[], &mrs);
}

fn zero_page(va: u64) {
    unsafe { ptr::write_bytes(va as *mut u8, 0, PAGE_SIZE as usize) };
}

fn ensure_alias_for_index(alloc: &mut Allocator, slot: usize) -> u64 {
    unsafe {
        let m = MAPPINGS[slot];
        if m.pid == 0 {
            log("xv6-host: alias for empty mapping\n");
            halt_loop();
        }
        if m.alias_slot == 0 {
            let alias_slot = alloc.copy_cap(m.frame_slot, cap_rights(false, false, true, true));
            page_map(alias_slot, INIT_VSPACE, m.alias_page, true, false);
            MAPPINGS[slot].alias_slot = alias_slot;
        }
        MAPPINGS[slot].alias_page
    }
}

fn zero_frame_with_temporary_alias(alloc: &mut Allocator, frame_slot: u64, alias_va: u64) {
    let alias_slot = alloc.copy_cap(frame_slot, cap_rights(false, false, true, true));
    page_map(alias_slot, INIT_VSPACE, alias_va, true, false);
    zero_page(alias_va);
    page_unmap(alias_slot);
    alloc.delete_cap_slot(alias_slot);
}

fn is_lazy_heap_addr(child: &Child, va: u64) -> bool {
    va >= child.heap_start && va < child.brk && va < CHILD_HEAP_LIMIT
}
