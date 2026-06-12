#![no_std]
#![deny(unsafe_attr_outside_unsafe)]
#![deny(unsafe_op_in_unsafe_fn)]

use core::arch::asm;
use core::cmp::min;
use core::fmt::{self, Write};
use core::ptr;
use core::sync::atomic::{AtomicPtr, Ordering};

pub mod rt;

#[doc(hidden)]
pub mod __log {
    pub use log_crate::{debug, error, info, trace, warn};
}

#[macro_export]
macro_rules! trace {
    ($($arg:tt)+) => {
        $crate::__log::trace!($($arg)+)
    };
}

#[macro_export]
macro_rules! debug {
    ($($arg:tt)+) => {
        $crate::__log::debug!($($arg)+)
    };
}

#[macro_export]
macro_rules! info {
    ($($arg:tt)+) => {
        $crate::__log::info!($($arg)+)
    };
}

#[macro_export]
macro_rules! warn {
    ($($arg:tt)+) => {
        $crate::__log::warn!($($arg)+)
    };
}

#[macro_export]
macro_rules! error {
    ($($arg:tt)+) => {
        $crate::__log::error!($($arg)+)
    };
}

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
pub const SYS_SEND: isize = -5;
pub const SYS_NB_RECV: isize = -8;
pub const SYS_RECV: isize = -7;
pub const SYS_WAIT: isize = -9;
pub const SYS_NB_WAIT: isize = -10;
pub const SYS_YIELD: isize = -11;
pub const SYS_DEBUG_PUT_CHAR: isize = -12;
pub const SYS_DEBUG_HALT: isize = -14;

pub const LABEL_UNTYPED_RETYPE: u64 = 1;
pub const LABEL_TCB_READ_REGISTERS: u64 = 2;
pub const LABEL_TCB_WRITE_REGISTERS: u64 = 3;
pub const LABEL_TCB_CONFIGURE: u64 = 5;
pub const LABEL_TCB_SET_PRIORITY: u64 = 6;
pub const LABEL_TCB_SET_SCHED_PARAMS: u64 = 8;
pub const LABEL_TCB_SUSPEND: u64 = 12;
pub const LABEL_TCB_BIND_NOTIFICATION: u64 = 14;
pub const LABEL_TCB_SET_FLAGS: u64 = 17;
pub const LABEL_CNODE_REVOKE: u64 = 18;
pub const LABEL_CNODE_DELETE: u64 = 19;
pub const LABEL_CNODE_COPY: u64 = 21;
pub const LABEL_CNODE_MINT: u64 = 22;
pub const LABEL_CNODE_SAVE_CALLER: u64 = 255;
pub const LABEL_IRQ_ISSUE_IRQ_HANDLER: u64 = 26;
pub const LABEL_IRQ_ACK: u64 = 27;
pub const LABEL_IRQ_SET_NOTIFICATION: u64 = 28;
pub const LABEL_SCHED_CONTROL_CONFIGURE_FLAGS: u64 = 33;
pub const LABEL_RISCV_PAGE_MAP: u64 = 41;
pub const LABEL_RISCV_PAGE_UNMAP: u64 = 42;
pub const LABEL_RISCV_PAGE_GET_ADDRESS: u64 = 43;
pub const LABEL_RISCV_ASID_POOL_ASSIGN: u64 = 45;

pub const OBJ_UNTYPED: u64 = 0;
pub const OBJ_TCB: u64 = 1;
pub const OBJ_ENDPOINT: u64 = 2;
pub const OBJ_NOTIFICATION: u64 = 3;
pub const OBJ_CAP_TABLE: u64 = 4;
pub const OBJ_SCHED_CONTEXT: u64 = 5;
pub const OBJ_REPLY: u64 = 6;
pub const OBJ_GIGA_PAGE: u64 = 7;
pub const OBJ_4K: u64 = 8;
pub const OBJ_MEGA_PAGE: u64 = 9;
pub const OBJ_PAGE_TABLE: u64 = 10;

pub const FAULT_UNKNOWN_SYSCALL: u64 = 2;
pub const FAULT_USER_EXCEPTION: u64 = 3;
pub const FAULT_VM_FAULT: u64 = 6;

pub const USER_EXCEPTION_FAULT_IP: usize = 0;
pub const USER_EXCEPTION_SP: usize = 1;
pub const USER_EXCEPTION_NUMBER: usize = 2;
pub const USER_EXCEPTION_CODE: usize = 3;
pub const USER_EXCEPTION_LENGTH: usize = 4;

pub const KERNEL_TIMER_IRQ: u64 = 96;

pub const TCB_FLAG_NO_FLAG: u64 = 0x0;
pub const TCB_FLAG_FPU_DISABLED: u64 = 0x1;
pub const TCB_FLAG_MASK: u64 = TCB_FLAG_NO_FLAG | TCB_FLAG_FPU_DISABLED;

#[derive(Copy, Clone, Debug, Eq, PartialEq)]
pub struct TcbSetFlagsResult {
    pub error: u64,
    pub flags: u64,
}

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
    pub schedcontrol: SlotRegion,
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
    /// Badge returned by the receive syscall. For endpoint messages, this is
    /// the badge minted onto the sender's capability; for notifications, this
    /// contains the delivered notification badge bits.
    pub badge: u64,
    /// seL4 message info/tag word, containing the message label, extra-cap
    /// count, and message-register length. Use `msg_label()` and `msg_len()`
    /// to decode it.
    pub info: u64,
    /// Snapshot of the message registers. The first 4 MRs come from syscall
    /// return registers and later MRs come from the IPC buffer; the valid entry
    /// count is given by `msg_len(info)`.
    pub mrs: [u64; 64],
}

static IPC_BUFFER: AtomicPtr<IpcBuffer> = AtomicPtr::new(ptr::null_mut());

pub fn init_ipc_buffer(addr: u64) {
    IPC_BUFFER.store(addr as *mut IpcBuffer, Ordering::Release);
}

#[inline]
fn ipc_buffer_ptr() -> *mut IpcBuffer {
    IPC_BUFFER.load(Ordering::Acquire)
}

pub unsafe fn sel4_call(service: u64, info: u64, mrs: &[u64]) -> IpcMessage {
    unsafe {
        let ipc = &mut *ipc_buffer_ptr();
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
            inlateout("a7") SYS_CALL => _,
            clobber_abi("C"),
            options(nostack)
        );
        read_ipc_message(a0, a1, a2, a3, a4, a5)
    }
}

pub unsafe fn sel4_recv(ep: u64) -> IpcMessage {
    unsafe { sel4_wait(ep) }
}

pub unsafe fn sel4_recv_with_reply(ep: u64, reply: u64) -> IpcMessage {
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
            inlateout("a6") reply => _,
            inlateout("a7") SYS_RECV => _,
            clobber_abi("C"),
            options(nostack)
        );
        read_ipc_message(a0, a1, a2, a3, a4, a5)
    }
}

pub unsafe fn sel4_nb_recv(ep: u64) -> IpcMessage {
    unsafe { sel4_nb_wait(ep) }
}

pub unsafe fn sel4_nb_recv_with_reply(ep: u64, reply: u64) -> IpcMessage {
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
            inlateout("a6") reply => _,
            inlateout("a7") SYS_NB_RECV => _,
            clobber_abi("C"),
            options(nostack)
        );
        read_ipc_message(a0, a1, a2, a3, a4, a5)
    }
}

pub unsafe fn sel4_wait(ep: u64) -> IpcMessage {
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
            inlateout("a7") SYS_WAIT => _,
            clobber_abi("C"),
            options(nostack)
        );
        read_ipc_message(a0, a1, a2, a3, a4, a5)
    }
}

pub unsafe fn sel4_nb_wait(ep: u64) -> IpcMessage {
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
            inlateout("a7") SYS_NB_WAIT => _,
            clobber_abi("C"),
            options(nostack)
        );
        read_ipc_message(a0, a1, a2, a3, a4, a5)
    }
}

pub unsafe fn sel4_reply_recv(ep: u64, info: u64, reply_mrs: &[u64]) -> IpcMessage {
    let _ = (info, reply_mrs);
    unsafe { sel4_wait(ep) }
}

pub unsafe fn sel4_reply_recv_with_reply(
    ep: u64,
    info: u64,
    reply_mrs: &[u64],
    reply: u64,
) -> IpcMessage {
    unsafe {
        let ipc = &mut *ipc_buffer_ptr();
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
            inlateout("a6") reply => _,
            inlateout("a7") SYS_REPLY_RECV => _,
            clobber_abi("C"),
            options(nostack)
        );
        read_ipc_message(a0, a1, a2, a3, a4, a5)
    }
}

pub unsafe fn sel4_send(dest: u64, info: u64, mrs: &[u64]) {
    unsafe {
        let ipc = &mut *ipc_buffer_ptr();
        let mut i = 4;
        while i < mrs.len() {
            ipc.msg[i] = mrs[i];
            i += 1;
        }
        asm!(
            "ecall",
            inlateout("a0") dest => _,
            inlateout("a1") info => _,
            inlateout("a2") mr(mrs, 0) => _,
            inlateout("a3") mr(mrs, 1) => _,
            inlateout("a4") mr(mrs, 2) => _,
            inlateout("a5") mr(mrs, 3) => _,
            inlateout("a7") SYS_SEND => _,
            clobber_abi("C"),
            options(nostack)
        );
    }
}

pub unsafe fn sel4_yield() {
    unsafe {
        asm!(
            "ecall",
            inlateout("a7") SYS_YIELD => _,
            clobber_abi("C"),
            options(nostack)
        );
    }
}

pub fn call_checked(service: u64, label: u64, extra_caps: &[u64], mrs: &[u64]) {
    unsafe {
        let ipc = &mut *ipc_buffer_ptr();
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
            log_crate::error!("sel4-user: seL4 call failed label={} err={}", label, err);
            halt_loop();
        }
    }
}

/// Set or clear seL4 TCB feature flags.
///
/// Passing zero for both `clear` and `set` reads the current flags, matching
/// upstream seL4's `TCB_SetFlags` contract.
/// `flags` is valid when `error == 0`.
pub fn sel4_tcb_set_flags(tcb: u64, clear: u64, set: u64) -> TcbSetFlagsResult {
    unsafe {
        let reply = sel4_call(tcb, msg_info(LABEL_TCB_SET_FLAGS, 0, 0, 2), &[clear, set]);
        let err = msg_label(reply.info);
        if err == 0 {
            TcbSetFlagsResult {
                error: err,
                flags: reply.mrs[0],
            }
        } else {
            TcbSetFlagsResult {
                error: err,
                flags: 0,
            }
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
        let ipc = &*ipc_buffer_ptr();
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
    unsafe { (*ipc_buffer_ptr()).msg[i] }
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
            inlateout("a0") ch as u64 => _,
            inlateout("a7") SYS_DEBUG_PUT_CHAR => _,
            clobber_abi("C"),
            options(nostack)
        );
    }
}

static LOGGER: UserLogger = UserLogger;

struct UserLogger;

struct DebugWriter;

pub fn init_logger() {
    let _ = log_crate::set_logger(&LOGGER);
    log_crate::set_max_level(max_level());
}

impl log_crate::Log for UserLogger {
    fn enabled(&self, metadata: &log_crate::Metadata<'_>) -> bool {
        metadata.level() <= log_crate::max_level()
    }

    fn log(&self, record: &log_crate::Record<'_>) {
        if self.enabled(record.metadata()) {
            let mut writer = DebugWriter;
            let _ = writer.write_fmt(*record.args());
            let _ = writer.write_str("\n");
        }
    }

    fn flush(&self) {}
}

impl Write for DebugWriter {
    fn write_str(&mut self, s: &str) -> fmt::Result {
        for b in s.as_bytes() {
            putchar(*b);
        }
        Ok(())
    }
}

fn max_level() -> log_crate::LevelFilter {
    match env!("SEL4_LOG_LEVEL") {
        "off" => log_crate::LevelFilter::Off,
        "error" => log_crate::LevelFilter::Error,
        "warn" => log_crate::LevelFilter::Warn,
        "debug" => log_crate::LevelFilter::Debug,
        "trace" => log_crate::LevelFilter::Trace,
        _ => log_crate::LevelFilter::Info,
    }
}

pub struct LogBytes<'a>(pub &'a [u8]);

impl fmt::Display for LogBytes<'_> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        for &byte in self.0 {
            if (0x20..=0x7e).contains(&byte) {
                f.write_char(byte as char)?;
            } else {
                write!(f, "\\x{byte:02x}")?;
            }
        }
        Ok(())
    }
}

pub fn halt_loop() -> ! {
    unsafe {
        asm!(
            "ecall",
            inlateout("a7") SYS_DEBUG_HALT => _,
            clobber_abi("C"),
            options(nostack)
        );
    }
    loop {
        unsafe { sel4_yield() };
    }
}
