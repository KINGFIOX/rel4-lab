use core::cell::UnsafeCell;
use core::cmp::min;
use core::ptr;

use crate::allocator::Allocator;
use crate::consts::*;
use crate::types::{Mapping, TaskStruct};
use crate::util::*;
use sel4_user::{
    call_checked, cap_rights, cnode_cap_data, msg_info, msg_label, read_ipc_mr, sel4_call,
};

const EMPTY_MAPPING: Mapping = Mapping {
    pid: 0,
    child_page: 0,
    alias_page: 0,
    frame_slot: 0,
    alias_slot: 0,
    writable: false,
    executable: false,
    pool_frame: false,
};

#[derive(Copy, Clone)]
struct FramePoolEntry {
    frame_slot: u64,
}

const EMPTY_FRAME_POOL_ENTRY: FramePoolEntry = FramePoolEntry { frame_slot: 0 };

struct ChildMemoryState {
    mappings: [Mapping; MAX_MAPPINGS],
    mapping_count: usize,
    frame_pool: [FramePoolEntry; FRAME_POOL_CAP],
    frame_pool_len: usize,
}

impl ChildMemoryState {
    const fn new() -> Self {
        Self {
            mappings: [EMPTY_MAPPING; MAX_MAPPINGS],
            mapping_count: 0,
            frame_pool: [EMPTY_FRAME_POOL_ENTRY; FRAME_POOL_CAP],
            frame_pool_len: 0,
        }
    }
}

struct ChildMemory {
    state: UnsafeCell<ChildMemoryState>,
}

// xv6-host handles all process memory operations from the single rootserver
// fault loop, so mapping and recycled-frame tables are serialized there.
unsafe impl Sync for ChildMemory {}

impl ChildMemory {
    const fn new() -> Self {
        Self {
            state: UnsafeCell::new(ChildMemoryState::new()),
        }
    }

    fn mapping_count(&self) -> usize {
        unsafe { (&*self.state.get()).mapping_count }
    }

    fn mapping(&self, slot: usize) -> Mapping {
        unsafe { (&*self.state.get()).mappings[slot] }
    }

    fn set_mapping(&self, slot: usize, mapping: Mapping) {
        unsafe {
            (&mut *self.state.get()).mappings[slot] = mapping;
        }
    }

    fn set_mapping_alias_slot(&self, slot: usize, alias_slot: u64) {
        unsafe {
            (&mut *self.state.get()).mappings[slot].alias_slot = alias_slot;
        }
    }

    fn allocate_mapping_slot(&self) -> Option<(usize, u64)> {
        let state = unsafe { &mut *self.state.get() };
        let mut slot = state.mapping_count;
        let mut i = 0usize;
        while i < state.mapping_count {
            if state.mappings[i].pid == 0 {
                slot = i;
                break;
            }
            i += 1;
        }
        if slot >= MAX_MAPPINGS {
            return None;
        }
        let alias = if slot == state.mapping_count {
            let alias = HOST_ALIAS_BASE + (state.mapping_count as u64) * PAGE_SIZE;
            state.mapping_count += 1;
            alias
        } else {
            state.mappings[slot].alias_page
        };
        Some((slot, alias))
    }

    fn mapping_slots_available(&self) -> usize {
        let state = unsafe { &*self.state.get() };
        let mut free = MAX_MAPPINGS.saturating_sub(state.mapping_count);
        let mut i = 0usize;
        while i < state.mapping_count {
            if state.mappings[i].pid == 0 {
                free += 1;
            }
            i += 1;
        }
        free
    }

    fn frame_pool_len(&self) -> usize {
        unsafe { (&*self.state.get()).frame_pool_len }
    }

    fn take_frame(&self) -> Option<u64> {
        let state = unsafe { &mut *self.state.get() };
        if state.frame_pool_len == 0 {
            return None;
        }
        state.frame_pool_len -= 1;
        let index = state.frame_pool_len;
        let frame_slot = state.frame_pool[index].frame_slot;
        state.frame_pool[index] = EMPTY_FRAME_POOL_ENTRY;
        Some(frame_slot)
    }

    fn push_frame(&self, frame_slot: u64) -> bool {
        let state = unsafe { &mut *self.state.get() };
        let len = state.frame_pool_len;
        if len >= FRAME_POOL_CAP {
            return false;
        }
        state.frame_pool[len] = FramePoolEntry { frame_slot };
        state.frame_pool_len = len + 1;
        true
    }
}

static CHILD_MEMORY: ChildMemory = ChildMemory::new();

pub(crate) fn create_child(
    alloc: &mut Allocator,
    proc_slot: usize,
    pid: u64,
    parent_pid: u64,
    fault_ep: u64,
) -> TaskStruct {
    let untyped = alloc.process_untyped(proc_slot);
    create_child_from_untyped(alloc, pid, parent_pid, fault_ep, untyped)
}

pub(crate) fn create_child_from_untyped(
    alloc: &mut Allocator,
    pid: u64,
    parent_pid: u64,
    fault_ep: u64,
    untyped: u64,
) -> TaskStruct {
    let tcb = alloc.retype_one_from(untyped, OBJ_TCB, 0);
    let cnode = alloc.retype_one_from(untyped, OBJ_CAP_TABLE, CHILD_CNODE_BITS);
    let vspace = alloc.retype_one_from(untyped, OBJ_PAGE_TABLE, 0);
    let ipc_frame = alloc.retype_one_from(untyped, OBJ_4K, 0);
    let sched_context = alloc.retype_one_from(untyped, OBJ_SCHED_CONTEXT, 0);
    let fault_ep_cap = alloc.mint_cap(fault_ep, cap_rights(true, true, true, true), pid);

    call_checked(INIT_ASID_POOL, LABEL_ASID_POOL_ASSIGN, &[vspace], &[]);
    page_map(ipc_frame, vspace, CHILD_IPC_BUFFER, true, false);

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
    let mrs = [cspace_data, 0, CHILD_IPC_BUFFER];
    call_checked(tcb, LABEL_TCB_CONFIGURE, &[cnode, vspace, ipc_frame], &mrs);
    alloc.configure_sched_context(sched_context, pid);
    call_checked(
        tcb,
        LABEL_TCB_SET_SCHED_PARAMS,
        &[INIT_TCB, sched_context, fault_ep_cap],
        &[CHILD_MCP, CHILD_PRIORITY],
    );

    TaskStruct {
        pid,
        parent_pid,
        state: PROC_RUNNABLE,
        exit_status: 0,
        reparented_to_init: false,
        tcb,
        cnode,
        vspace,
        ipc_frame,
        sched_context,
        untyped,
        fault_ep,
        fault_ep_cap,
        entry: 0,
        brk: 0,
        heap_start: 0,
        heap_mapped_end: 0,
        sparse_reserved: 0,
        cwd: {
            let mut cwd = [0u8; MAX_PATH_BYTES];
            cwd[0] = b'/';
            cwd
        },
        cwd_len: 1,
        cwd_inode: 0,
        fds: [MAX_OPEN_FILES; MAX_FD],
        fd_serial: [false; MAX_FD],
        wait_status_ptr: 0,
        wait_reply_slot: 0,
        wait_reply_mrs: [0; 11],
        vfs_reply_slot: 0,
        vfs_reply_mrs: [0; 11],
        vfs_fd: 0,
        vfs_buf: 0,
        vfs_len: 0,
        vfs_done: 0,
        sleep_deadline: 0,
        sleep_reply_slot: 0,
        sleep_reply_mrs: [0; 11],
        deferred_reply_slot: 0,
        deferred_mrs: [0; 64],
    }
}

pub(crate) fn destroy_child_objects(alloc: &mut Allocator, child: &TaskStruct) {
    if child.pid == 0 {
        return;
    }
    if child.tcb != 0 {
        call_checked(child.tcb, LABEL_TCB_SUSPEND, &[], &[]);
    }
    alloc.delete_cap_slot(child.tcb);
    alloc.delete_cap_slot(child.ipc_frame);
    alloc.delete_cap_slot(child.sched_context);
    alloc.delete_cap_slot(child.fault_ep_cap);
    alloc.delete_cap_slot(child.cnode);
    alloc.delete_cap_slot(child.vspace);
    alloc.revoke_cap_slot(child.untyped);
}

pub(crate) fn load_payload(alloc: &mut Allocator, child: &mut TaskStruct) {
    load_elf(alloc, child, PAYLOAD_ELF);
}

pub(crate) fn elf_image_valid(elf: &[u8]) -> bool {
    if elf.len() < 64 || &elf[0..4] != b"\x7fELF" || elf[4] != 2 || elf[5] != 1 {
        return false;
    }
    let phoff = read_u64(elf, 32) as usize;
    let phentsize = read_u16(elf, 54) as usize;
    let phnum = read_u16(elf, 56) as usize;
    if phentsize < 56 {
        return false;
    }

    let mut i = 0usize;
    while i < phnum {
        let Some(off) = phoff.checked_add(i.saturating_mul(phentsize)) else {
            return false;
        };
        if off.checked_add(56).is_none_or(|end| end > elf.len()) {
            return false;
        }
        if read_u32(elf, off) == 1 {
            let p_offset = read_u64(elf, off + 8) as usize;
            let p_filesz = read_u64(elf, off + 32) as usize;
            if p_offset
                .checked_add(p_filesz)
                .is_none_or(|end| end > elf.len())
            {
                return false;
            }
        }
        i += 1;
    }
    true
}

pub(crate) fn load_elf(alloc: &mut Allocator, child: &mut TaskStruct, elf: &[u8]) {
    if !elf_image_valid(elf) {
        warn!("xv6-host: bad payload ELF");
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
            warn!("xv6-host: truncated program headers");
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
            warn!("xv6-host: segment outside payload");
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
            warn!("xv6-host: failed to copy payload");
            halt_loop();
        }
        image_end = image_end.max(p_vaddr.saturating_add(p_memsz));
    }

    child.entry = entry;
    child.brk = align_up(image_end);
    child.heap_start = child.brk;
    child.heap_mapped_end = child.brk;
    info!("xv6-host: payload entry={:#x} brk={:#x}", entry, child.brk);
}

pub(crate) fn reset_process_mappings(alloc: &mut Allocator, pid: u64) {
    let mut i = 0;
    while i < CHILD_MEMORY.mapping_count() {
        let mapping = CHILD_MEMORY.mapping(i);
        if mapping.pid == pid && mapping.child_page != CHILD_IPC_BUFFER {
            unmap_mapping_at(alloc, i);
        }
        i += 1;
    }
}

pub(crate) fn clear_process_mappings(alloc: &mut Allocator, pid: u64) {
    let mut i = 0;
    while i < CHILD_MEMORY.mapping_count() {
        if CHILD_MEMORY.mapping(i).pid == pid {
            unmap_mapping_at(alloc, i);
        }
        i += 1;
    }
}

pub(crate) fn unmap_child_range(alloc: &mut Allocator, pid: u64, start: u64, end: u64) {
    let start = align_down(start);
    let end = align_up(end);
    let mut i = 0;
    while i < CHILD_MEMORY.mapping_count() {
        let mapping = CHILD_MEMORY.mapping(i);
        if mapping.pid == pid && mapping.child_page >= start && mapping.child_page < end {
            unmap_mapping_at(alloc, i);
        }
        i += 1;
    }
}

pub(crate) fn mapping_slots_available() -> usize {
    CHILD_MEMORY.mapping_slots_available()
}

pub(crate) fn frame_pool_available() -> usize {
    CHILD_MEMORY.frame_pool_len()
}

pub(crate) fn map_stack(alloc: &mut Allocator, child: &mut TaskStruct) {
    map_stack_pages(alloc, child, CHILD_STACK_PAGES);
}

pub(crate) fn map_stack_pages(alloc: &mut Allocator, child: &mut TaskStruct, pages: usize) {
    let guard_base = align_up(child.brk);
    let stack_top = guard_base + ((pages as u64 + 1) * PAGE_SIZE);
    for i in 0..pages {
        let va = stack_top - ((i as u64 + 1) * PAGE_SIZE);
        map_fresh_child_page(alloc, child, va, true, false);
    }
    child.brk = stack_top;
    child.heap_start = stack_top;
    child.heap_mapped_end = stack_top;
}

pub(crate) fn start_child(child: &TaskStruct) {
    start_child_with_a0(child, 0);
}

pub(crate) fn start_child_with_a0(child: &TaskStruct, a0: u64) {
    start_child_with_a0_a1(child, a0, 0);
}

pub(crate) fn start_child_with_a0_a1(child: &TaskStruct, a0: u64, a1: u64) {
    let mut ctx = [0u64; USER_CONTEXT_WORDS];
    ctx[0] = child.entry;
    ctx[2] = child.heap_start;
    ctx[16] = a0;
    ctx[17] = a1;
    write_user_context(child.tcb, &ctx, true);
}

pub(crate) fn mint_cap_to_child(
    child: &TaskStruct,
    dst_cptr: u64,
    src_cap: u64,
    rights: u64,
    badge: u64,
) {
    let mrs = [
        dst_cptr,
        CHILD_CNODE_BITS,
        src_cap,
        ROOT_CNODE_DEPTH,
        rights,
        badge,
    ];
    call_checked(child.cnode, LABEL_CNODE_MINT, &[ROOT_CNODE], &mrs);
}

pub(crate) fn map_fresh_child_page(
    alloc: &mut Allocator,
    child: &TaskStruct,
    child_va: u64,
    writable: bool,
    executable: bool,
) -> u64 {
    let page = align_down(child_va);
    if let Some(idx) = lookup_mapping_index(child.pid, page) {
        let alias = ensure_alias_for_index(alloc, idx);
        return alias;
    }
    let (frame_slot, needs_zero) = alloc_process_frame(alloc);
    map_existing_frame(
        alloc,
        child.pid,
        frame_slot,
        child.vspace,
        page,
        writable,
        executable,
        true,
        true,
        needs_zero,
    )
}

pub(crate) fn map_lazy_child_page(
    alloc: &mut Allocator,
    child: &TaskStruct,
    child_va: u64,
    writable: bool,
    executable: bool,
) -> u64 {
    let page = align_down(child_va);
    if let Some(idx) = lookup_mapping_index(child.pid, page) {
        return CHILD_MEMORY.mapping(idx).alias_page;
    }
    let (frame_slot, needs_zero) = alloc_process_frame(alloc);
    map_existing_frame(
        alloc,
        child.pid,
        frame_slot,
        child.vspace,
        page,
        writable,
        executable,
        false,
        true,
        needs_zero,
    )
}

pub(crate) fn map_existing_child_frame(
    alloc: &mut Allocator,
    child: &TaskStruct,
    frame_slot: u64,
    child_va: u64,
    writable: bool,
    executable: bool,
) -> u64 {
    map_existing_frame(
        alloc,
        child.pid,
        frame_slot,
        child.vspace,
        child_va,
        writable,
        executable,
        false,
        false,
        false,
    )
}

pub(crate) fn frame_paddr(frame_slot: u64) -> u64 {
    let reply = unsafe { sel4_call(frame_slot, msg_info(LABEL_PAGE_GET_ADDRESS, 0, 0, 0), &[]) };
    let err = msg_label(reply.info);
    if err != 0 {
        warn!("xv6-host: Page_GetAddress failed err={}", err);
        halt_loop();
    }
    reply.mrs[0]
}

fn map_existing_frame(
    alloc: &mut Allocator,
    pid: u64,
    frame_slot: u64,
    vspace: u64,
    child_va: u64,
    writable: bool,
    executable: bool,
    with_alias: bool,
    pool_frame: bool,
    zero_frame: bool,
) -> u64 {
    let (mapping_slot, alias_va) =
        register_mapping(pid, child_va, frame_slot, writable, executable, pool_frame);
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
    call_checked(frame_slot, LABEL_PAGE_MAP, &[vspace], &[va, rights, attrs]);
}

fn register_mapping(
    pid: u64,
    child_page: u64,
    frame_slot: u64,
    writable: bool,
    executable: bool,
    pool_frame: bool,
) -> (usize, u64) {
    let Some((slot, alias)) = CHILD_MEMORY.allocate_mapping_slot() else {
        warn!("xv6-host: mapping table full");
        halt_loop();
    };
    CHILD_MEMORY.set_mapping(
        slot,
        Mapping {
            pid,
            child_page: align_down(child_page),
            alias_page: alias,
            frame_slot,
            alias_slot: 0,
            writable,
            executable,
            pool_frame,
        },
    );
    (slot, alias)
}

fn unmap_mapping_at(alloc: &mut Allocator, slot: usize) {
    let mapping = CHILD_MEMORY.mapping(slot);
    if mapping.pid == 0 {
        return;
    }
    if mapping.alias_slot != 0 {
        page_unmap(mapping.alias_slot);
    }
    if mapping.frame_slot != 0 {
        page_unmap(mapping.frame_slot);
    }
    alloc.delete_cap_slot(mapping.alias_slot);
    if mapping.pool_frame {
        release_process_frame(alloc, mapping.frame_slot);
    } else {
        alloc.delete_cap_slot(mapping.frame_slot);
    }
    CHILD_MEMORY.set_mapping(
        slot,
        Mapping {
            pid: 0,
            child_page: 0,
            alias_page: mapping.alias_page,
            frame_slot: 0,
            alias_slot: 0,
            writable: false,
            executable: false,
            pool_frame: false,
        },
    );
}

fn page_unmap(frame_slot: u64) {
    call_checked(frame_slot, LABEL_PAGE_UNMAP, &[], &[]);
}

pub(crate) fn is_child_page_mapped(child: &TaskStruct, va: u64) -> bool {
    lookup_mapping(child.pid, va).is_some()
}

fn lookup_mapping_index(pid: u64, child_page: u64) -> Option<usize> {
    let page = align_down(child_page);
    let mut i = 0;
    while i < CHILD_MEMORY.mapping_count() {
        let mapping = CHILD_MEMORY.mapping(i);
        if mapping.pid == pid && mapping.child_page == page {
            return Some(i);
        }
        i += 1;
    }
    None
}

fn lookup_mapping(pid: u64, child_page: u64) -> Option<Mapping> {
    lookup_mapping_index(pid, child_page).map(|index| CHILD_MEMORY.mapping(index))
}

fn alloc_process_frame(alloc: &mut Allocator) -> (u64, bool) {
    if let Some(frame_slot) = CHILD_MEMORY.take_frame() {
        return (frame_slot, true);
    }
    (alloc.retype_one(OBJ_4K, 0), false)
}

fn release_process_frame(alloc: &mut Allocator, frame_slot: u64) {
    if frame_slot == 0 {
        return;
    }
    if !CHILD_MEMORY.push_frame(frame_slot) {
        alloc.delete_cap_slot(frame_slot);
    }
}

fn child_ptr(
    alloc: &mut Allocator,
    child: &TaskStruct,
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
    let mapping = CHILD_MEMORY.mapping(idx);
    if write && !mapping.writable {
        return None;
    }
    let alias_page = ensure_alias_for_index(alloc, idx);
    Some((alias_page + off) as *mut u8)
}

pub(crate) fn copy_from_child(
    alloc: &mut Allocator,
    child: &TaskStruct,
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

pub(crate) fn copy_to_child(
    alloc: &mut Allocator,
    child: &TaskStruct,
    va: u64,
    src: &[u8],
) -> bool {
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

pub(crate) fn copy_to_child_raw(
    alloc: &mut Allocator,
    child: &TaskStruct,
    va: u64,
    src: &[u8],
) -> bool {
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
    child: &TaskStruct,
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

pub(crate) fn clone_address_space(alloc: &mut Allocator, parent: &TaskStruct, child: &TaskStruct) {
    let mut i = 0;
    let live_heap_end = align_up(parent.brk);
    while i < CHILD_MEMORY.mapping_count() {
        let mapping = CHILD_MEMORY.mapping(i);
        let freed_heap_page =
            mapping.child_page >= live_heap_end && mapping.child_page < CHILD_HEAP_LIMIT;
        if mapping.pid == parent.pid && mapping.child_page != CHILD_IPC_BUFFER && !freed_heap_page {
            let src_alias = ensure_alias_for_index(alloc, i);
            let dst_alias = map_fresh_child_page(
                alloc,
                child,
                mapping.child_page,
                mapping.writable,
                mapping.executable,
            );
            unsafe {
                ptr::copy_nonoverlapping(
                    src_alias as *const u8,
                    dst_alias as *mut u8,
                    PAGE_SIZE as usize,
                );
            }
        }
        i += 1;
    }
}

pub(crate) fn clone_page_count(parent: &TaskStruct) -> usize {
    let mut count = 0usize;
    let mut i = 0;
    let live_heap_end = align_up(parent.brk);
    while i < CHILD_MEMORY.mapping_count() {
        let mapping = CHILD_MEMORY.mapping(i);
        let freed_heap_page =
            mapping.child_page >= live_heap_end && mapping.child_page < CHILD_HEAP_LIMIT;
        if mapping.pid == parent.pid && mapping.child_page != CHILD_IPC_BUFFER && !freed_heap_page {
            count += 1;
        }
        i += 1;
    }
    count
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
        warn!("xv6-host: TCB_ReadRegisters failed err={}", err);
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
    let mapping = CHILD_MEMORY.mapping(slot);
    if mapping.pid == 0 {
        warn!("xv6-host: alias for empty mapping");
        halt_loop();
    }
    if mapping.alias_slot == 0 {
        let alias_slot = alloc.copy_cap(mapping.frame_slot, cap_rights(false, false, true, true));
        page_map(alias_slot, INIT_VSPACE, mapping.alias_page, true, false);
        CHILD_MEMORY.set_mapping_alias_slot(slot, alias_slot);
    }
    mapping.alias_page
}

fn zero_frame_with_temporary_alias(alloc: &mut Allocator, frame_slot: u64, alias_va: u64) {
    let alias_slot = alloc.copy_cap(frame_slot, cap_rights(false, false, true, true));
    page_map(alias_slot, INIT_VSPACE, alias_va, true, false);
    zero_page(alias_va);
    page_unmap(alias_slot);
    alloc.delete_cap_slot(alias_slot);
}

fn is_lazy_heap_addr(child: &TaskStruct, va: u64) -> bool {
    va >= child.heap_start && va < child.brk && va < CHILD_HEAP_LIMIT
}
