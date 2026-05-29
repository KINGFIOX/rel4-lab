#![no_std]
#![no_main]
#![deny(unsafe_attr_outside_unsafe)]
#![deny(unsafe_op_in_unsafe_fn)]

use core::arch::asm;
use core::cmp::min;
use core::panic::PanicInfo;
use core::ptr;

const PAYLOAD_ELF: &[u8] = include_bytes!(env!("XV6_PAYLOAD_ELF"));
const README_BYTES: &[u8] = include_bytes!("../../../third_party/xv6-riscv/README");

const PAGE_SIZE: u64 = 4096;
const ROOT_CNODE: u64 = 2;
const INIT_TCB: u64 = 1;
const INIT_VSPACE: u64 = 3;
const IRQ_CONTROL: u64 = 4;
const INIT_ASID_POOL: u64 = 6;
const ROOT_CNODE_DEPTH: u64 = 64;
const WORD_BITS: u64 = 64;

const SYS_CALL: isize = -1;
const SYS_REPLY_RECV: isize = -2;
const SYS_RECV: isize = -5;
const SYS_YIELD: isize = -7;
const SYS_DEBUG_PUT_CHAR: isize = -9;
const SYS_DEBUG_HALT: isize = -11;

const LABEL_UNTYPED_RETYPE: u64 = 1;
const LABEL_TCB_WRITE_REGISTERS: u64 = 3;
const LABEL_TCB_CONFIGURE: u64 = 5;
const LABEL_TCB_SET_PRIORITY: u64 = 6;
const LABEL_TCB_BIND_NOTIFICATION: u64 = 13;
const LABEL_CNODE_COPY: u64 = 20;
const LABEL_CNODE_MINT: u64 = 21;
const LABEL_IRQ_ISSUE_IRQ_HANDLER: u64 = 26;
const LABEL_IRQ_SET_NOTIFICATION: u64 = 28;
const LABEL_RISCV_PAGE_MAP: u64 = 35;
const LABEL_RISCV_ASID_POOL_ASSIGN: u64 = 39;

const OBJ_TCB: u64 = 1;
const OBJ_ENDPOINT: u64 = 2;
const OBJ_NOTIFICATION: u64 = 3;
const OBJ_CAP_TABLE: u64 = 4;
const OBJ_4K: u64 = 6;
const OBJ_PAGE_TABLE: u64 = 8;

const CHILD_CNODE_BITS: u64 = 8;
const CHILD_FAULT_EP: u64 = 1;
const CHILD_IPC_BUFFER: u64 = 0x7000_0000;
const CHILD_STACK_TOP: u64 = 0x7001_0000;
const CHILD_STACK_PAGES: usize = 16;
const CHILD_HEAP_LIMIT: u64 = 0x7800_0000;
const HOST_ALIAS_BASE: u64 = 0x4000_0000;
const MAX_MAPPINGS: usize = 768;
const MAX_FD: usize = 32;
const KERNEL_TIMER_IRQ: u64 = 96;

const FAULT_UNKNOWN_SYSCALL: u64 = 2;

const SYS_FORK: u64 = 1;
const SYS_EXIT: u64 = 2;
const SYS_WAIT: u64 = 3;
const SYS_READ: u64 = 5;
const SYS_FSTAT: u64 = 8;
const SYS_DUP: u64 = 10;
const SYS_GETPID: u64 = 11;
const SYS_SBRK: u64 = 12;
const SYS_PAUSE: u64 = 13;
const SYS_UPTIME: u64 = 14;
const SYS_OPEN: u64 = 15;
const SYS_WRITE: u64 = 16;
const SYS_CLOSE: u64 = 21;

const T_DIR: u16 = 1;
const T_FILE: u16 = 2;
const T_DEVICE: u16 = 3;
const ROOT_INO: u32 = 1;
const README_INO: u32 = 2;
const CONSOLE_INO: u32 = 3;

#[repr(C)]
struct SlotRegion {
    start: u64,
    end: u64,
}

#[repr(C)]
#[derive(Copy, Clone)]
struct UntypedDesc {
    paddr: u64,
    size_bits: u8,
    is_device: u8,
    _padding: [u8; 6],
}

#[repr(C)]
struct BootInfo {
    extra_len: u64,
    node_id: u64,
    num_nodes: u64,
    num_io_pt_levels: u64,
    ipc_buffer: u64,
    empty: SlotRegion,
    shared_frames: SlotRegion,
    user_image_frames: SlotRegion,
    user_image_paging: SlotRegion,
    io_space_caps: SlotRegion,
    extra_bi_pages: SlotRegion,
    init_thread_cnode_size_bits: u64,
    init_thread_domain: u8,
    _pad_domain: [u8; 7],
    untyped: SlotRegion,
    untyped_list: [UntypedDesc; 230],
}

#[repr(C)]
struct IpcBuffer {
    tag: u64,
    msg: [u64; 120],
    user_data: u64,
    caps_or_badges: [u64; 3],
    receive_cnode: u64,
    receive_index: u64,
    receive_depth: u64,
}

#[derive(Copy, Clone)]
struct Mapping {
    child_page: u64,
    alias_page: u64,
}

#[derive(Copy, Clone)]
struct FdEntry {
    kind: u8,
    offset: usize,
}

impl FdEntry {
    const fn closed() -> Self {
        Self {
            kind: FD_CLOSED,
            offset: 0,
        }
    }
}

const FD_CLOSED: u8 = 0;
const FD_CONSOLE: u8 = 1;
const FD_README: u8 = 2;
const FD_ROOTDIR: u8 = 3;

struct Allocator {
    next_slot: u64,
    empty_end: u64,
    untyped_slot: u64,
}

struct Child {
    tcb: u64,
    vspace: u64,
    fault_ep: u64,
    entry: u64,
    brk: u64,
    heap_mapped_end: u64,
}

struct IpcMessage {
    info: u64,
    mrs: [u64; 16],
}

static mut IPC_BUFFER: *mut IpcBuffer = ptr::null_mut();
static mut MAPPINGS: [Mapping; MAX_MAPPINGS] = [Mapping {
    child_page: 0,
    alias_page: 0,
}; MAX_MAPPINGS];
static mut MAPPING_COUNT: usize = 0;
static mut FD_TABLE: [FdEntry; MAX_FD] = [FdEntry::closed(); MAX_FD];
static mut TICKS: u64 = 0;
static mut SAW_FAULT_IPC: bool = false;

const ROOT_DIRENTS: [u8; 64] = [
    1, 0, b'.', 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 1, 0, b'.', b'.', 0, 0, 0, 0, 0, 0, 0, 0, 0,
    0, 0, 0, 2, 0, b'R', b'E', b'A', b'D', b'M', b'E', 0, 0, 0, 0, 0, 0, 0, 0, 3, 0, b'c', b'o',
    b'n', b's', b'o', b'l', b'e', 0, 0, 0, 0, 0, 0, 0,
];

#[unsafe(no_mangle)]
pub extern "C" fn _start(bootinfo: usize) -> ! {
    unsafe {
        clear_bss();
    }
    run(bootinfo as *const BootInfo);
}

unsafe fn clear_bss() {
    unsafe extern "C" {
        static mut __bss_start: u8;
        static mut __bss_end: u8;
    }
    unsafe {
        let start = core::ptr::addr_of_mut!(__bss_start) as usize;
        let end = core::ptr::addr_of_mut!(__bss_end) as usize;
        ptr::write_bytes(start as *mut u8, 0, end.saturating_sub(start));
    }
}

fn run(bi_ptr: *const BootInfo) -> ! {
    let bi = unsafe { &*bi_ptr };
    unsafe { IPC_BUFFER = bi.ipc_buffer as *mut IpcBuffer };
    log("xv6-host: boot\n");

    let mut alloc = Allocator::new(bi);
    init_fds();
    let mut child = create_child(&mut alloc);
    setup_timer_notification(&mut alloc);
    load_payload(&mut alloc, &mut child);
    map_stack(&mut alloc, &child);
    start_child(&child);

    log("xv6-host: waiting for fault IPC\n");
    let mut reply_pending = false;
    let mut reply_mrs = [0u64; 11];
    loop {
        let msg = if reply_pending {
            reply_pending = false;
            unsafe { sel4_reply_recv(child.fault_ep, msg_info(0, 0, 0, 11), &reply_mrs) }
        } else {
            unsafe { sel4_recv(child.fault_ep) }
        };

        let label = msg_label(msg.info);
        if label == 0 {
            unsafe {
                TICKS = TICKS.wrapping_add(1);
            }
            continue;
        }
        if label != FAULT_UNKNOWN_SYSCALL {
            log("xv6-host: non-syscall fault label=");
            print_u64(label);
            log("\n");
            halt_loop();
        }

        unsafe {
            if !SAW_FAULT_IPC {
                SAW_FAULT_IPC = true;
                log("xv6-host: UnknownSyscall fault IPC\n");
            }
        }

        reply_mrs = msg.mrs[..11].try_into().unwrap_or([0; 11]);
        let ret = handle_xv6_syscall(&mut alloc, &mut child, &msg.mrs);
        reply_mrs[0] = msg.mrs[0].wrapping_add(4);
        reply_mrs[3] = ret as u64;
        reply_pending = true;
    }
}

fn setup_timer_notification(alloc: &mut Allocator) {
    let ntfn = alloc.retype_one(OBJ_NOTIFICATION, 0);
    let irq_handler = alloc.alloc_slot();
    call_checked(
        IRQ_CONTROL,
        LABEL_IRQ_ISSUE_IRQ_HANDLER,
        &[ROOT_CNODE],
        &[KERNEL_TIMER_IRQ, irq_handler, ROOT_CNODE_DEPTH],
    );
    call_checked(irq_handler, LABEL_IRQ_SET_NOTIFICATION, &[ntfn], &[]);
    call_checked(INIT_TCB, LABEL_TCB_BIND_NOTIFICATION, &[ntfn], &[]);
}

impl Allocator {
    fn new(bi: &BootInfo) -> Self {
        let mut selected = 0;
        let start = bi.untyped.start as usize;
        let end = bi.untyped.end as usize;
        let mut slot = bi.untyped.start;
        for i in start..end {
            let desc = bi.untyped_list[i - start];
            if desc.is_device == 0 && desc.size_bits >= 24 {
                selected = slot;
                break;
            }
            slot += 1;
        }
        if selected == 0 {
            log("xv6-host: no usable untyped\n");
            halt_loop();
        }
        Self {
            next_slot: bi.empty.start,
            empty_end: bi.empty.end,
            untyped_slot: selected,
        }
    }

    fn alloc_slot(&mut self) -> u64 {
        if self.next_slot >= self.empty_end {
            log("xv6-host: out of CSpace slots\n");
            halt_loop();
        }
        let slot = self.next_slot;
        self.next_slot += 1;
        slot
    }

    fn retype_one(&mut self, ty: u64, user_size: u64) -> u64 {
        let slot = self.alloc_slot();
        let mrs = [ty, user_size, 0, 0, slot, 1];
        call_checked(self.untyped_slot, LABEL_UNTYPED_RETYPE, &[ROOT_CNODE], &mrs);
        slot
    }

    fn copy_cap(&mut self, src_slot: u64, rights: u64) -> u64 {
        let dst = self.alloc_slot();
        let mrs = [dst, ROOT_CNODE_DEPTH, src_slot, ROOT_CNODE_DEPTH, rights];
        call_checked(ROOT_CNODE, LABEL_CNODE_COPY, &[ROOT_CNODE], &mrs);
        dst
    }
}

fn create_child(alloc: &mut Allocator) -> Child {
    let tcb = alloc.retype_one(OBJ_TCB, 0);
    let cnode = alloc.retype_one(OBJ_CAP_TABLE, CHILD_CNODE_BITS);
    let vspace = alloc.retype_one(OBJ_PAGE_TABLE, 0);
    let fault_ep = alloc.retype_one(OBJ_ENDPOINT, 0);
    let ipc_frame = alloc.retype_one(OBJ_4K, 0);

    call_checked(INIT_ASID_POOL, LABEL_RISCV_ASID_POOL_ASSIGN, &[vspace], &[]);
    map_existing_frame(alloc, ipc_frame, vspace, CHILD_IPC_BUFFER, true, false);

    let fault_ep_child_cap = {
        let mrs = [
            CHILD_FAULT_EP,
            CHILD_CNODE_BITS,
            fault_ep,
            ROOT_CNODE_DEPTH,
            cap_rights(true, true, true, true),
            0,
        ];
        call_checked(cnode, LABEL_CNODE_MINT, &[ROOT_CNODE], &mrs);
        CHILD_FAULT_EP
    };

    let cspace_data = cnode_cap_data(0, WORD_BITS - CHILD_CNODE_BITS);
    let mrs = [fault_ep_child_cap, cspace_data, 0, CHILD_IPC_BUFFER];
    call_checked(tcb, LABEL_TCB_CONFIGURE, &[cnode, vspace, ipc_frame], &mrs);
    call_checked(tcb, LABEL_TCB_SET_PRIORITY, &[INIT_TCB], &[254]);

    Child {
        tcb,
        vspace,
        fault_ep,
        entry: 0,
        brk: 0,
        heap_mapped_end: 0,
    }
}

fn load_payload(alloc: &mut Allocator, child: &mut Child) {
    let elf = PAYLOAD_ELF;
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
            map_fresh_child_page(alloc, child.vspace, page, true, true);
            page += PAGE_SIZE;
        }
        if p_filesz > 0 {
            if !copy_to_child(p_vaddr, &elf[p_offset..p_offset + p_filesz]) {
                log("xv6-host: failed to copy payload\n");
                halt_loop();
            }
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

fn map_stack(alloc: &mut Allocator, child: &Child) {
    for i in 0..CHILD_STACK_PAGES {
        let va = CHILD_STACK_TOP - ((i as u64 + 1) * PAGE_SIZE);
        map_fresh_child_page(alloc, child.vspace, va, true, false);
    }
}

fn start_child(child: &Child) {
    let mut regs = [0u64; 34];
    regs[0] = 1; // resume target
    regs[1] = 34;
    regs[2] = child.entry;
    regs[3] = 0; // ra
    regs[4] = CHILD_STACK_TOP; // sp, works around current TCB_WriteRegisters mr loop
    regs[18] = 0; // a0
    regs[19] = 0; // a1
    call_checked(child.tcb, LABEL_TCB_WRITE_REGISTERS, &[], &regs);
}

fn map_existing_frame(
    alloc: &mut Allocator,
    frame_slot: u64,
    vspace: u64,
    child_va: u64,
    writable: bool,
    executable: bool,
) -> u64 {
    let alias_slot = alloc.copy_cap(frame_slot, cap_rights(false, false, true, true));
    let alias_va = register_mapping(child_va);
    page_map(alias_slot, INIT_VSPACE, alias_va, true, false);
    zero_page(alias_va);
    page_map(frame_slot, vspace, child_va, writable, executable);
    alias_va
}

fn map_fresh_child_page(
    alloc: &mut Allocator,
    vspace: u64,
    child_va: u64,
    writable: bool,
    executable: bool,
) -> u64 {
    let page = align_down(child_va);
    if let Some(alias) = lookup_alias(page) {
        return alias;
    }
    let frame_slot = alloc.retype_one(OBJ_4K, 0);
    map_existing_frame(alloc, frame_slot, vspace, page, writable, executable)
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

fn register_mapping(child_page: u64) -> u64 {
    unsafe {
        if MAPPING_COUNT >= MAX_MAPPINGS {
            log("xv6-host: mapping table full\n");
            halt_loop();
        }
        let alias = HOST_ALIAS_BASE + (MAPPING_COUNT as u64) * PAGE_SIZE;
        MAPPINGS[MAPPING_COUNT] = Mapping {
            child_page: align_down(child_page),
            alias_page: alias,
        };
        MAPPING_COUNT += 1;
        alias
    }
}

fn lookup_alias(child_page: u64) -> Option<u64> {
    unsafe {
        let page = align_down(child_page);
        let mut i = 0;
        while i < MAPPING_COUNT {
            let m = MAPPINGS[i];
            if m.child_page == page {
                return Some(m.alias_page);
            }
            i += 1;
        }
        None
    }
}

fn child_ptr(va: u64) -> Option<*mut u8> {
    let page = align_down(va);
    let off = va - page;
    lookup_alias(page).map(|alias| (alias + off) as *mut u8)
}

fn copy_from_child(va: u64, out: &mut [u8]) -> bool {
    let mut done = 0usize;
    while done < out.len() {
        let cur = va + done as u64;
        let page_left = (PAGE_SIZE - (cur & (PAGE_SIZE - 1))) as usize;
        let n = min(page_left, out.len() - done);
        let Some(src) = child_ptr(cur) else {
            return false;
        };
        unsafe { ptr::copy_nonoverlapping(src as *const u8, out[done..].as_mut_ptr(), n) };
        done += n;
    }
    true
}

fn copy_to_child(va: u64, src: &[u8]) -> bool {
    let mut done = 0usize;
    while done < src.len() {
        let cur = va + done as u64;
        let page_left = (PAGE_SIZE - (cur & (PAGE_SIZE - 1))) as usize;
        let n = min(page_left, src.len() - done);
        let Some(dst) = child_ptr(cur) else {
            return false;
        };
        unsafe { ptr::copy_nonoverlapping(src[done..].as_ptr(), dst, n) };
        done += n;
    }
    true
}

fn copy_cstr_from_child(va: u64, out: &mut [u8]) -> Option<usize> {
    for i in 0..out.len() {
        let mut b = [0u8; 1];
        if !copy_from_child(va + i as u64, &mut b) {
            return None;
        }
        out[i] = b[0];
        if b[0] == 0 {
            return Some(i);
        }
    }
    None
}

fn handle_xv6_syscall(alloc: &mut Allocator, child: &mut Child, mrs: &[u64; 16]) -> i64 {
    let sysno = mrs[10];
    let a0 = mrs[3];
    let a1 = mrs[4];
    let a2 = mrs[5];
    unsafe {
        TICKS = TICKS.wrapping_add(1);
    }

    match sysno {
        SYS_EXIT => {
            log("xv6-host: exit(");
            print_i64(a0 as i64);
            log(")\n");
            halt_loop();
        }
        SYS_WRITE => sys_write(a0 as usize, a1, a2 as usize),
        SYS_READ => sys_read(a0 as usize, a1, a2 as usize),
        SYS_OPEN => sys_open(a0),
        SYS_CLOSE => sys_close(a0 as usize),
        SYS_DUP => sys_dup(a0 as usize),
        SYS_FSTAT => sys_fstat(a0 as usize, a1),
        SYS_SBRK => sys_sbrk(alloc, child, a0 as i64),
        SYS_GETPID => 1,
        SYS_UPTIME => unsafe { TICKS as i64 },
        SYS_PAUSE => {
            unsafe { sel4_yield() };
            0
        }
        SYS_FORK | SYS_WAIT => -1,
        _ => -1,
    }
}

fn sys_write(fd: usize, buf: u64, len: usize) -> i64 {
    if len == 0 {
        return 0;
    }
    if fd >= MAX_FD || unsafe { FD_TABLE[fd].kind } != FD_CONSOLE || fd == 0 {
        return -1;
    }
    let mut scratch = [0u8; 128];
    let mut done = 0usize;
    while done < len {
        let n = min(scratch.len(), len - done);
        if !copy_from_child(buf + done as u64, &mut scratch[..n]) {
            return -1;
        }
        for b in &scratch[..n] {
            putchar(*b);
        }
        done += n;
    }
    len as i64
}

fn sys_read(fd: usize, dst: u64, len: usize) -> i64 {
    if fd >= MAX_FD || len == 0 {
        return if fd < MAX_FD { 0 } else { -1 };
    }
    unsafe {
        let entry = FD_TABLE[fd];
        match entry.kind {
            FD_CONSOLE => 0,
            FD_README => {
                let data = README_BYTES;
                if entry.offset >= data.len() {
                    return 0;
                }
                let n = min(len, data.len() - entry.offset);
                if !copy_to_child(dst, &data[entry.offset..entry.offset + n]) {
                    return -1;
                }
                FD_TABLE[fd].offset += n;
                n as i64
            }
            FD_ROOTDIR => {
                let data = &ROOT_DIRENTS;
                if entry.offset >= data.len() {
                    return 0;
                }
                let n = min(len, data.len() - entry.offset);
                if !copy_to_child(dst, &data[entry.offset..entry.offset + n]) {
                    return -1;
                }
                FD_TABLE[fd].offset += n;
                n as i64
            }
            _ => -1,
        }
    }
}

fn sys_open(path_ptr: u64) -> i64 {
    let mut path = [0u8; 128];
    let Some(len) = copy_cstr_from_child(path_ptr, &mut path) else {
        return -1;
    };
    let name = basename(&path[..len]);
    let kind = if path_is_root(&path[..len]) || name == b"." || name == b".." {
        FD_ROOTDIR
    } else if name == b"README" {
        FD_README
    } else if name == b"console" {
        FD_CONSOLE
    } else {
        return -1;
    };
    alloc_fd(kind)
}

fn sys_close(fd: usize) -> i64 {
    if fd >= MAX_FD {
        return -1;
    }
    unsafe {
        if fd <= 2 {
            FD_TABLE[fd].offset = 0;
            return 0;
        }
        if FD_TABLE[fd].kind == FD_CLOSED {
            return -1;
        }
        FD_TABLE[fd] = FdEntry::closed();
    }
    0
}

fn sys_dup(fd: usize) -> i64 {
    if fd >= MAX_FD {
        return -1;
    }
    let entry = unsafe { FD_TABLE[fd] };
    if entry.kind == FD_CLOSED {
        return -1;
    }
    unsafe {
        for i in 0..MAX_FD {
            if FD_TABLE[i].kind == FD_CLOSED {
                FD_TABLE[i] = entry;
                return i as i64;
            }
        }
    }
    -1
}

fn sys_fstat(fd: usize, dst: u64) -> i64 {
    if fd >= MAX_FD {
        return -1;
    }
    let entry = unsafe { FD_TABLE[fd] };
    let (typ, ino, size) = match entry.kind {
        FD_CONSOLE => (T_DEVICE, CONSOLE_INO, 0u64),
        FD_README => (T_FILE, README_INO, README_BYTES.len() as u64),
        FD_ROOTDIR => (T_DIR, ROOT_INO, ROOT_DIRENTS.len() as u64),
        _ => return -1,
    };
    let mut st = [0u8; 24];
    write_i32(&mut st, 0, 1);
    write_u32(&mut st, 4, ino);
    write_u16(&mut st, 8, typ);
    write_u16(&mut st, 10, 1);
    write_u64_bytes(&mut st, 16, size);
    if !copy_to_child(dst, &st) {
        return -1;
    }
    0
}

fn sys_sbrk(alloc: &mut Allocator, child: &mut Child, increment: i64) -> i64 {
    let old = child.brk;
    let new_brk = if increment >= 0 {
        old.saturating_add(increment as u64)
    } else {
        old.saturating_sub((-increment) as u64)
    };
    if new_brk > CHILD_HEAP_LIMIT {
        return -1;
    }
    if new_brk > child.heap_mapped_end {
        let mut page = align_up(child.heap_mapped_end);
        while page < align_up(new_brk) {
            map_fresh_child_page(alloc, child.vspace, page, true, false);
            page += PAGE_SIZE;
        }
        child.heap_mapped_end = align_up(new_brk);
    }
    child.brk = new_brk;
    old as i64
}

fn init_fds() {
    unsafe {
        let mut i = 0;
        while i < MAX_FD {
            FD_TABLE[i] = FdEntry::closed();
            i += 1;
        }
        FD_TABLE[0] = FdEntry {
            kind: FD_CONSOLE,
            offset: 0,
        };
        FD_TABLE[1] = FdEntry {
            kind: FD_CONSOLE,
            offset: 0,
        };
        FD_TABLE[2] = FdEntry {
            kind: FD_CONSOLE,
            offset: 0,
        };
    }
}

fn alloc_fd(kind: u8) -> i64 {
    unsafe {
        for i in 0..MAX_FD {
            if FD_TABLE[i].kind == FD_CLOSED {
                FD_TABLE[i] = FdEntry { kind, offset: 0 };
                return i as i64;
            }
        }
    }
    -1
}

fn basename(path: &[u8]) -> &[u8] {
    let mut start = 0;
    for (i, b) in path.iter().enumerate() {
        if *b == b'/' {
            start = i + 1;
        }
    }
    &path[start..]
}

fn path_is_root(path: &[u8]) -> bool {
    path.is_empty() || path == b"/" || path == b"."
}

unsafe fn sel4_call(service: u64, info: u64, mrs: &[u64]) -> IpcMessage {
    unsafe {
        let ipc = &mut *IPC_BUFFER;
        let mut i = 4;
        while i < mrs.len() {
            ipc.msg[i] = mrs[i];
            i += 1;
        }
        let mut a0 = service;
        let mut a1 = info;
        let mut a2 = mr(mrs, 0);
        let mut a3 = mr(mrs, 1);
        let mut a4 = mr(mrs, 2);
        let mut a5 = mr(mrs, 3);
        asm!(
            "ecall",
            inlateout("a0") a0,
            inlateout("a1") a1,
            inlateout("a2") a2,
            inlateout("a3") a3,
            inlateout("a4") a4,
            inlateout("a5") a5,
            in("a7") SYS_CALL,
            options(nostack)
        );
        read_ipc_message(a0, a1, a2, a3, a4, a5)
    }
}

unsafe fn sel4_recv(ep: u64) -> IpcMessage {
    unsafe {
        let mut a0 = ep;
        let mut a1 = 0u64;
        let mut a2 = 0u64;
        let mut a3 = 0u64;
        let mut a4 = 0u64;
        let mut a5 = 0u64;
        asm!(
            "ecall",
            inlateout("a0") a0,
            inlateout("a1") a1,
            inlateout("a2") a2,
            inlateout("a3") a3,
            inlateout("a4") a4,
            inlateout("a5") a5,
            in("a7") SYS_RECV,
            options(nostack)
        );
        read_ipc_message(a0, a1, a2, a3, a4, a5)
    }
}

unsafe fn sel4_reply_recv(ep: u64, info: u64, reply_mrs: &[u64]) -> IpcMessage {
    unsafe {
        let ipc = &mut *IPC_BUFFER;
        let mut i = 4;
        while i < reply_mrs.len() {
            ipc.msg[i] = reply_mrs[i];
            i += 1;
        }
        let mut a0 = ep;
        let mut a1 = info;
        let mut a2 = mr(reply_mrs, 0);
        let mut a3 = mr(reply_mrs, 1);
        let mut a4 = mr(reply_mrs, 2);
        let mut a5 = mr(reply_mrs, 3);
        asm!(
            "ecall",
            inlateout("a0") a0,
            inlateout("a1") a1,
            inlateout("a2") a2,
            inlateout("a3") a3,
            inlateout("a4") a4,
            inlateout("a5") a5,
            in("a7") SYS_REPLY_RECV,
            options(nostack)
        );
        read_ipc_message(a0, a1, a2, a3, a4, a5)
    }
}

unsafe fn sel4_yield() {
    unsafe {
        asm!("ecall", in("a7") SYS_YIELD, options(nostack));
    }
}

fn call_checked(service: u64, label: u64, extra_caps: &[u64], mrs: &[u64]) {
    unsafe {
        let ipc = &mut *IPC_BUFFER;
        for i in 0..3 {
            ipc.caps_or_badges[i] = if i < extra_caps.len() {
                extra_caps[i]
            } else {
                0
            };
        }
        let reply = sel4_call(
            service,
            msg_info(label, 0, extra_caps.len() as u64, mrs.len() as u64),
            mrs,
        );
        let err = msg_label(reply.info);
        if err != 0 {
            log("xv6-host: seL4 call failed label=");
            print_u64(label);
            log(" err=");
            print_u64(err);
            log("\n");
            halt_loop();
        }
    }
}

unsafe fn read_ipc_message(
    _badge: u64,
    info: u64,
    a2: u64,
    a3: u64,
    a4: u64,
    a5: u64,
) -> IpcMessage {
    unsafe {
        let ipc = &*IPC_BUFFER;
        let mut mrs = [0u64; 16];
        mrs[0] = a2;
        mrs[1] = a3;
        mrs[2] = a4;
        mrs[3] = a5;
        let len = min(msg_len(info) as usize, mrs.len());
        let mut i = 4;
        while i < len {
            mrs[i] = ipc.msg[i];
            i += 1;
        }
        IpcMessage { info, mrs }
    }
}

fn mr(mrs: &[u64], i: usize) -> u64 {
    if i < mrs.len() { mrs[i] } else { 0 }
}

fn msg_info(label: u64, caps_unwrapped: u64, extra_caps: u64, length: u64) -> u64 {
    ((label & 0x000f_ffff_ffff_ffff) << 12)
        | ((caps_unwrapped & 0x7) << 9)
        | ((extra_caps & 0x3) << 7)
        | (length & 0x7f)
}

fn msg_label(info: u64) -> u64 {
    (info >> 12) & 0x000f_ffff_ffff_ffff
}

fn msg_len(info: u64) -> u64 {
    info & 0x7f
}

fn cap_rights(grant_reply: bool, grant: bool, read: bool, write: bool) -> u64 {
    ((grant_reply as u64) << 3) | ((grant as u64) << 2) | ((read as u64) << 1) | write as u64
}

fn cnode_cap_data(guard: u64, guard_size: u64) -> u64 {
    (guard << 6) | (guard_size & 0x3f)
}

fn zero_page(va: u64) {
    unsafe { ptr::write_bytes(va as *mut u8, 0, PAGE_SIZE as usize) };
}

fn align_down(v: u64) -> u64 {
    v & !(PAGE_SIZE - 1)
}

fn align_up(v: u64) -> u64 {
    (v + PAGE_SIZE - 1) & !(PAGE_SIZE - 1)
}

fn read_u16(buf: &[u8], off: usize) -> u16 {
    u16::from_le_bytes([buf[off], buf[off + 1]])
}

fn read_u32(buf: &[u8], off: usize) -> u32 {
    u32::from_le_bytes([buf[off], buf[off + 1], buf[off + 2], buf[off + 3]])
}

fn read_u64(buf: &[u8], off: usize) -> u64 {
    u64::from_le_bytes([
        buf[off],
        buf[off + 1],
        buf[off + 2],
        buf[off + 3],
        buf[off + 4],
        buf[off + 5],
        buf[off + 6],
        buf[off + 7],
    ])
}

fn write_u16(buf: &mut [u8], off: usize, v: u16) {
    buf[off..off + 2].copy_from_slice(&v.to_le_bytes());
}

fn write_i32(buf: &mut [u8], off: usize, v: i32) {
    buf[off..off + 4].copy_from_slice(&v.to_le_bytes());
}

fn write_u32(buf: &mut [u8], off: usize, v: u32) {
    buf[off..off + 4].copy_from_slice(&v.to_le_bytes());
}

fn write_u64_bytes(buf: &mut [u8], off: usize, v: u64) {
    buf[off..off + 8].copy_from_slice(&v.to_le_bytes());
}

fn putchar(ch: u8) {
    unsafe {
        asm!("ecall", in("a0") ch as u64, in("a7") SYS_DEBUG_PUT_CHAR, options(nostack));
    }
}

fn log(s: &str) {
    for b in s.as_bytes() {
        putchar(*b);
    }
}

fn print_u64(mut n: u64) {
    if n == 0 {
        putchar(b'0');
        return;
    }
    let mut buf = [0u8; 20];
    let mut i = 0;
    while n > 0 {
        buf[i] = b'0' + (n % 10) as u8;
        n /= 10;
        i += 1;
    }
    while i > 0 {
        i -= 1;
        putchar(buf[i]);
    }
}

fn print_i64(n: i64) {
    if n < 0 {
        putchar(b'-');
        print_u64(n.wrapping_neg() as u64);
    } else {
        print_u64(n as u64);
    }
}

fn print_hex(mut n: u64) {
    log("0x");
    if n == 0 {
        putchar(b'0');
        return;
    }
    let mut buf = [0u8; 16];
    let mut i = 0;
    while n > 0 {
        let d = (n & 0xf) as u8;
        buf[i] = if d < 10 { b'0' + d } else { b'a' + d - 10 };
        n >>= 4;
        i += 1;
    }
    while i > 0 {
        i -= 1;
        putchar(buf[i]);
    }
}

fn halt_loop() -> ! {
    unsafe {
        asm!("ecall", in("a7") SYS_DEBUG_HALT, options(nostack));
    }
    loop {
        unsafe { sel4_yield() };
    }
}

#[panic_handler]
fn panic(_info: &PanicInfo) -> ! {
    log("xv6-host: panic\n");
    halt_loop()
}
