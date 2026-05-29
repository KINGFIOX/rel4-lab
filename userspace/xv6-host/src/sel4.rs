use core::arch::asm;
use core::cmp::min;
use core::ptr;

use crate::consts::{SYS_CALL, SYS_RECV, SYS_REPLY_RECV, SYS_YIELD};
use crate::types::{IpcBuffer, IpcMessage};
use crate::util::{halt_loop, log, print_u64};

static mut IPC_BUFFER: *mut IpcBuffer = ptr::null_mut();

pub(crate) fn init_ipc_buffer(addr: u64) {
    unsafe { IPC_BUFFER = addr as *mut IpcBuffer };
}

pub(crate) unsafe fn sel4_call(service: u64, info: u64, mrs: &[u64]) -> IpcMessage {
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

pub(crate) unsafe fn sel4_recv(ep: u64) -> IpcMessage {
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

pub(crate) unsafe fn sel4_reply_recv(ep: u64, info: u64, reply_mrs: &[u64]) -> IpcMessage {
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

pub(crate) unsafe fn sel4_yield() {
    unsafe {
        asm!("ecall", in("a7") SYS_YIELD, options(nostack));
    }
}

pub(crate) fn call_checked(service: u64, label: u64, extra_caps: &[u64], mrs: &[u64]) {
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

pub(crate) fn msg_info(label: u64, caps_unwrapped: u64, extra_caps: u64, length: u64) -> u64 {
    ((label & 0x000f_ffff_ffff_ffff) << 12)
        | ((caps_unwrapped & 0x7) << 9)
        | ((extra_caps & 0x3) << 7)
        | (length & 0x7f)
}

pub(crate) fn msg_label(info: u64) -> u64 {
    (info >> 12) & 0x000f_ffff_ffff_ffff
}

fn msg_len(info: u64) -> u64 {
    info & 0x7f
}

pub(crate) fn cap_rights(grant_reply: bool, grant: bool, read: bool, write: bool) -> u64 {
    ((grant_reply as u64) << 3) | ((grant as u64) << 2) | ((read as u64) << 1) | write as u64
}

pub(crate) fn cnode_cap_data(guard: u64, guard_size: u64) -> u64 {
    (guard << 6) | (guard_size & 0x3f)
}
