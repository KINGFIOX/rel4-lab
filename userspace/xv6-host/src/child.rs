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
    pid: 0,
    child_page: 0,
    alias_page: 0,
    writable: false,
    executable: false,
}; MAX_MAPPINGS];
static mut MAPPING_COUNT: usize = 0;

pub(crate) fn create_child(
    alloc: &mut Allocator,
    pid: u64,
    parent_pid: u64,
    fault_ep: u64,
) -> Child {
    let tcb = alloc.retype_one(OBJ_TCB, 0);
    let cnode = alloc.retype_one(OBJ_CAP_TABLE, CHILD_CNODE_BITS);
    let vspace = alloc.retype_one(OBJ_PAGE_TABLE, 0);
    let ipc_frame = alloc.retype_one(OBJ_4K, 0);

    call_checked(INIT_ASID_POOL, LABEL_RISCV_ASID_POOL_ASSIGN, &[vspace], &[]);
    map_existing_frame(alloc, pid, ipc_frame, vspace, CHILD_IPC_BUFFER, true, false);

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
        parent_pid,
        state: PROC_RUNNABLE,
        exit_status: 0,
        tcb,
        vspace,
        fault_ep,
        entry: 0,
        brk: 0,
        heap_mapped_end: 0,
        fds: [crate::types::FdEntry::closed(); MAX_FD],
        cwd: FS_ROOT_NODE,
        wait_status_ptr: 0,
        wait_reply_slot: 0,
        wait_reply_mrs: [0; 11],
    }
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
        let p_offset = read_u64(elf, off + 8) as usize;
        let p_vaddr = read_u64(elf, off + 16);
        let p_filesz = read_u64(elf, off + 32) as usize;
        let p_memsz = read_u64(elf, off + 40);
        if p_offset.saturating_add(p_filesz) > elf.len() {
            log("xv6-host: segment outside payload\n");
            halt_loop();
        }

        let start = align_down(p_vaddr);
        let end = align_up(p_vaddr.saturating_add(p_memsz));
        let mut page = start;
        while page < end {
            map_fresh_child_page(alloc, child, page, true, true);
            page += PAGE_SIZE;
        }
        if p_filesz > 0 && !copy_to_child(child, p_vaddr, &elf[p_offset..p_offset + p_filesz]) {
            log("xv6-host: failed to copy payload\n");
            halt_loop();
        }
        image_end = image_end.max(p_vaddr.saturating_add(p_memsz));
    }

    child.entry = entry;
    child.brk = align_up(image_end);
    child.heap_mapped_end = child.brk;
    log("xv6-host: payload entry=");
    print_hex(entry);
    log(" brk=");
    print_hex(child.brk);
    log("\n");
}

pub(crate) fn reset_process_mappings(pid: u64) {
    unsafe {
        let mut i = 0;
        while i < MAPPING_COUNT {
            if MAPPINGS[i].pid == pid && MAPPINGS[i].child_page != CHILD_IPC_BUFFER {
                MAPPINGS[i].pid = 0;
            }
            i += 1;
        }
    }
}

pub(crate) fn clear_process_mappings(pid: u64) {
    unsafe {
        let mut i = 0;
        while i < MAPPING_COUNT {
            if MAPPINGS[i].pid == pid {
                MAPPINGS[i].pid = 0;
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
    if let Some(alias) = lookup_alias(child.pid, page) {
        return alias;
    }
    let frame_slot = alloc.retype_one(OBJ_4K, 0);
    map_existing_frame(
        alloc,
        child.pid,
        frame_slot,
        child.vspace,
        page,
        writable,
        executable,
    )
}

fn map_existing_frame(
    alloc: &mut Allocator,
    pid: u64,
    frame_slot: u64,
    vspace: u64,
    child_va: u64,
    writable: bool,
    executable: bool,
) -> u64 {
    let alias_slot = alloc.copy_cap(frame_slot, cap_rights(false, false, true, true));
    let alias_va = register_mapping(pid, child_va, writable, executable);
    page_map(alias_slot, INIT_VSPACE, alias_va, true, false);
    zero_page(alias_va);
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

fn register_mapping(pid: u64, child_page: u64, writable: bool, executable: bool) -> u64 {
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
            pid,
            child_page: align_down(child_page),
            alias_page: alias,
            writable,
            executable,
        };
        alias
    }
}

fn lookup_alias(pid: u64, child_page: u64) -> Option<u64> {
    unsafe {
        let page = align_down(child_page);
        let mut i = 0;
        while i < MAPPING_COUNT {
            let m = MAPPINGS[i];
            if m.pid == pid && m.child_page == page {
                return Some(m.alias_page);
            }
            i += 1;
        }
        None
    }
}

fn child_ptr(child: &Child, va: u64) -> Option<*mut u8> {
    let page = align_down(va);
    let off = va - page;
    lookup_alias(child.pid, page).map(|alias| (alias + off) as *mut u8)
}

pub(crate) fn copy_from_child(child: &Child, va: u64, out: &mut [u8]) -> bool {
    let mut done = 0usize;
    while done < out.len() {
        let cur = va + done as u64;
        let page_left = (PAGE_SIZE - (cur & (PAGE_SIZE - 1))) as usize;
        let n = min(page_left, out.len() - done);
        let Some(src) = child_ptr(child, cur) else {
            return false;
        };
        unsafe { ptr::copy_nonoverlapping(src as *const u8, out[done..].as_mut_ptr(), n) };
        done += n;
    }
    true
}

pub(crate) fn copy_to_child(child: &Child, va: u64, src: &[u8]) -> bool {
    let mut done = 0usize;
    while done < src.len() {
        let cur = va + done as u64;
        let page_left = (PAGE_SIZE - (cur & (PAGE_SIZE - 1))) as usize;
        let n = min(page_left, src.len() - done);
        let Some(dst) = child_ptr(child, cur) else {
            return false;
        };
        unsafe { ptr::copy_nonoverlapping(src[done..].as_ptr(), dst, n) };
        done += n;
    }
    true
}

pub(crate) fn copy_cstr_from_child(child: &Child, va: u64, out: &mut [u8]) -> Option<usize> {
    for i in 0..out.len() {
        let mut b = [0u8; 1];
        if !copy_from_child(child, va + i as u64, &mut b) {
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
                let dst_alias =
                    map_fresh_child_page(alloc, child, m.child_page, m.writable, m.executable);
                ptr::copy_nonoverlapping(
                    m.alias_page as *const u8,
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
