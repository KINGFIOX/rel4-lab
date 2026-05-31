#![no_std]
#![deny(unsafe_attr_outside_unsafe)]
#![deny(unsafe_op_in_unsafe_fn)]

use core::arch::asm;
use core::cmp::min;
use core::ptr;

pub const PAGE_SIZE: u64 = 4096;
pub const ROOT_CNODE: u64 = 2;
pub const INIT_TCB: u64 = 1;
pub const INIT_VSPACE: u64 = 3;
pub const IRQ_CONTROL: u64 = 4;
pub const INIT_ASID_POOL: u64 = 6;
pub const ROOT_CNODE_DEPTH: u64 = 64;
pub const WORD_BITS: u64 = 64;

pub const SYS_CALL: isize = -1;
pub const SYS_REPLY_RECV: isize = -2;
pub const SYS_SEND: isize = -3;
pub const SYS_RECV: isize = -5;
pub const SYS_REPLY: isize = -6;
pub const SYS_YIELD: isize = -7;
pub const SYS_DEBUG_PUT_CHAR: isize = -9;
pub const SYS_DEBUG_HALT: isize = -11;
pub const SYS_DEBUG_GET_CHAR: isize = -16;

pub const LABEL_UNTYPED_RETYPE: u64 = 1;
pub const LABEL_TCB_READ_REGISTERS: u64 = 2;
pub const LABEL_TCB_WRITE_REGISTERS: u64 = 3;
pub const LABEL_TCB_CONFIGURE: u64 = 5;
pub const LABEL_TCB_SET_PRIORITY: u64 = 6;
pub const LABEL_TCB_SUSPEND: u64 = 11;
pub const LABEL_TCB_BIND_NOTIFICATION: u64 = 13;
pub const LABEL_CNODE_REVOKE: u64 = 17;
pub const LABEL_CNODE_DELETE: u64 = 18;
pub const LABEL_CNODE_COPY: u64 = 20;
pub const LABEL_CNODE_MINT: u64 = 21;
pub const LABEL_CNODE_SAVE_CALLER: u64 = 25;
pub const LABEL_IRQ_ISSUE_IRQ_HANDLER: u64 = 26;
pub const LABEL_IRQ_ACK: u64 = 27;
pub const LABEL_IRQ_SET_NOTIFICATION: u64 = 28;
pub const LABEL_RISCV_PAGE_MAP: u64 = 35;
pub const LABEL_RISCV_PAGE_UNMAP: u64 = 36;
pub const LABEL_RISCV_PAGE_GET_ADDRESS: u64 = 37;
pub const LABEL_RISCV_ASID_POOL_ASSIGN: u64 = 39;

pub const OBJ_UNTYPED: u64 = 0;
pub const OBJ_TCB: u64 = 1;
pub const OBJ_ENDPOINT: u64 = 2;
pub const OBJ_NOTIFICATION: u64 = 3;
pub const OBJ_CAP_TABLE: u64 = 4;
pub const OBJ_4K: u64 = 6;
pub const OBJ_PAGE_TABLE: u64 = 8;

pub const FAULT_UNKNOWN_SYSCALL: u64 = 2;
pub const FAULT_VM_FAULT: u64 = 5;

pub const KERNEL_TIMER_IRQ: u64 = 96;

#[repr(C)]
pub struct SlotRegion {
    pub start: u64,
    pub end: u64,
}

#[repr(C)]
#[derive(Copy, Clone)]
pub struct UntypedDesc {
    pub paddr: u64,
    pub size_bits: u8,
    pub is_device: u8,
    pub _padding: [u8; 6],
}

#[repr(C)]
pub struct BootInfo {
    pub extra_len: u64,
    pub node_id: u64,
    pub num_nodes: u64,
    pub num_io_pt_levels: u64,
    pub ipc_buffer: u64,
    pub empty: SlotRegion,
    pub shared_frames: SlotRegion,
    pub user_image_frames: SlotRegion,
    pub user_image_paging: SlotRegion,
    pub io_space_caps: SlotRegion,
    pub extra_bi_pages: SlotRegion,
    pub init_thread_cnode_size_bits: u64,
    pub init_thread_domain: u8,
    pub _pad_domain: [u8; 7],
    pub untyped: SlotRegion,
    pub untyped_list: [UntypedDesc; 230],
}

#[repr(C)]
pub struct IpcBuffer {
    pub tag: u64,
    pub msg: [u64; 120],
    pub user_data: u64,
    pub caps_or_badges: [u64; 3],
    pub receive_cnode: u64,
    pub receive_index: u64,
    pub receive_depth: u64,
}

#[derive(Copy, Clone)]
pub struct IpcMessage {
    pub badge: u64,
    pub info: u64,
    pub mrs: [u64; 64],
}

static mut IPC_BUFFER: *mut IpcBuffer = ptr::null_mut();

pub fn init_ipc_buffer(addr: u64) {
    unsafe { IPC_BUFFER = addr as *mut IpcBuffer };
}

pub unsafe fn sel4_call(service: u64, info: u64, mrs: &[u64]) -> IpcMessage {
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

pub unsafe fn sel4_recv(ep: u64) -> IpcMessage {
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

pub unsafe fn sel4_reply_recv(ep: u64, info: u64, reply_mrs: &[u64]) -> IpcMessage {
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

pub unsafe fn sel4_reply(info: u64, reply_mrs: &[u64]) {
    unsafe {
        let ipc = &mut *IPC_BUFFER;
        let mut i = 4;
        while i < reply_mrs.len() {
            ipc.msg[i] = reply_mrs[i];
            i += 1;
        }
        let a0 = 0u64;
        let a1 = info;
        let a2 = mr(reply_mrs, 0);
        let a3 = mr(reply_mrs, 1);
        let a4 = mr(reply_mrs, 2);
        let a5 = mr(reply_mrs, 3);
        asm!(
            "ecall",
            in("a0") a0,
            in("a1") a1,
            in("a2") a2,
            in("a3") a3,
            in("a4") a4,
            in("a5") a5,
            in("a7") SYS_REPLY,
            options(nostack)
        );
    }
}

pub unsafe fn sel4_send(dest: u64, info: u64, mrs: &[u64]) {
    unsafe {
        let ipc = &mut *IPC_BUFFER;
        let mut i = 4;
        while i < mrs.len() {
            ipc.msg[i] = mrs[i];
            i += 1;
        }
        let a0 = dest;
        let a1 = info;
        let a2 = mr(mrs, 0);
        let a3 = mr(mrs, 1);
        let a4 = mr(mrs, 2);
        let a5 = mr(mrs, 3);
        asm!(
            "ecall",
            in("a0") a0,
            in("a1") a1,
            in("a2") a2,
            in("a3") a3,
            in("a4") a4,
            in("a5") a5,
            in("a7") SYS_SEND,
            options(nostack)
        );
    }
}

pub unsafe fn sel4_yield() {
    unsafe {
        asm!("ecall", in("a7") SYS_YIELD, options(nostack));
    }
}

pub fn call_checked(service: u64, label: u64, extra_caps: &[u64], mrs: &[u64]) {
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
            log("sel4-user: seL4 call failed label=");
            print_u64(label);
            log(" err=");
            print_u64(err);
            log("\n");
            halt_loop();
        }
    }
}

unsafe fn read_ipc_message(
    badge: u64,
    info: u64,
    a2: u64,
    a3: u64,
    a4: u64,
    a5: u64,
) -> IpcMessage {
    unsafe {
        let ipc = &*IPC_BUFFER;
        let mut mrs = [0u64; 64];
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
        IpcMessage { badge, info, mrs }
    }
}

pub unsafe fn read_ipc_mr(i: usize) -> u64 {
    unsafe { (*IPC_BUFFER).msg[i] }
}

fn mr(mrs: &[u64], i: usize) -> u64 {
    if i < mrs.len() { mrs[i] } else { 0 }
}

pub fn msg_info(label: u64, caps_unwrapped: u64, extra_caps: u64, length: u64) -> u64 {
    ((label & 0x000f_ffff_ffff_ffff) << 12)
        | ((caps_unwrapped & 0x7) << 9)
        | ((extra_caps & 0x3) << 7)
        | (length & 0x7f)
}

pub fn msg_label(info: u64) -> u64 {
    (info >> 12) & 0x000f_ffff_ffff_ffff
}

pub fn msg_len(info: u64) -> u64 {
    info & 0x7f
}

pub fn cap_rights(grant_reply: bool, grant: bool, read: bool, write: bool) -> u64 {
    ((grant_reply as u64) << 3) | ((grant as u64) << 2) | ((read as u64) << 1) | write as u64
}

pub fn cnode_cap_data(guard: u64, guard_size: u64) -> u64 {
    (guard << 6) | (guard_size & 0x3f)
}

pub fn align_down(v: u64) -> u64 {
    v & !(PAGE_SIZE - 1)
}

pub fn align_up(v: u64) -> u64 {
    (v + PAGE_SIZE - 1) & !(PAGE_SIZE - 1)
}

pub fn read_u16(buf: &[u8], off: usize) -> u16 {
    u16::from_le_bytes([buf[off], buf[off + 1]])
}

pub fn read_u32(buf: &[u8], off: usize) -> u32 {
    u32::from_le_bytes([buf[off], buf[off + 1], buf[off + 2], buf[off + 3]])
}

pub fn read_u64(buf: &[u8], off: usize) -> u64 {
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

pub fn write_u16(buf: &mut [u8], off: usize, v: u16) {
    buf[off..off + 2].copy_from_slice(&v.to_le_bytes());
}

pub fn write_i32(buf: &mut [u8], off: usize, v: i32) {
    buf[off..off + 4].copy_from_slice(&v.to_le_bytes());
}

pub fn write_u32(buf: &mut [u8], off: usize, v: u32) {
    buf[off..off + 4].copy_from_slice(&v.to_le_bytes());
}

pub fn write_u64_bytes(buf: &mut [u8], off: usize, v: u64) {
    buf[off..off + 8].copy_from_slice(&v.to_le_bytes());
}

pub fn putchar(ch: u8) {
    unsafe {
        asm!(
            "ecall",
            in("a0") ch as u64,
            in("a7") SYS_DEBUG_PUT_CHAR,
            options(nostack)
        );
    }
}

pub fn getchar() -> i32 {
    let mut ret = 0u64;
    unsafe {
        asm!(
            "ecall",
            inlateout("a0") ret,
            in("a7") SYS_DEBUG_GET_CHAR,
            options(nostack)
        );
    }
    ret as i64 as i32
}

pub fn log(s: &str) {
    for b in s.as_bytes() {
        putchar(*b);
    }
}

pub fn log_bytes(bytes: &[u8]) {
    for b in bytes {
        putchar(*b);
    }
}

pub fn print_u64(mut n: u64) {
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

pub fn print_i64(n: i64) {
    if n < 0 {
        putchar(b'-');
        print_u64(n.wrapping_neg() as u64);
    } else {
        print_u64(n as u64);
    }
}

pub fn print_hex(mut n: u64) {
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

pub fn halt_loop() -> ! {
    unsafe {
        asm!("ecall", in("a7") SYS_DEBUG_HALT, options(nostack));
    }
    loop {
        unsafe { sel4_yield() };
    }
}
