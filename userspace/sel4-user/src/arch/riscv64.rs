use core::arch::asm;

use crate::{SYS_CALL, SYS_DEBUG_HALT, SYS_DEBUG_PUT_CHAR, SYS_REPLY_RECV, SYS_SEND, SYS_YIELD};

#[inline(always)]
pub(crate) unsafe fn call(
    service: u64,
    info: u64,
    mr0: u64,
    mr1: u64,
    mr2: u64,
    mr3: u64,
) -> (u64, u64, u64, u64, u64, u64) {
    let mut a0 = service;
    let mut a1 = info;
    let mut a2 = mr0;
    let mut a3 = mr1;
    let mut a4 = mr2;
    let mut a5 = mr3;
    unsafe {
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
    }
    (a0, a1, a2, a3, a4, a5)
}

#[inline(always)]
pub(crate) unsafe fn recv_with_reply(
    ep: u64,
    reply: u64,
    syscall: isize,
) -> (u64, u64, u64, u64, u64, u64) {
    let mut a0 = ep;
    let mut a1 = 0u64;
    let mut a2 = 0u64;
    let mut a3 = 0u64;
    let mut a4 = 0u64;
    let mut a5 = 0u64;
    unsafe {
        asm!(
            "ecall",
            inlateout("a0") a0,
            inlateout("a1") a1,
            inlateout("a2") a2,
            inlateout("a3") a3,
            inlateout("a4") a4,
            inlateout("a5") a5,
            inlateout("a6") reply => _,
            inlateout("a7") syscall => _,
            clobber_abi("C"),
            options(nostack)
        );
    }
    (a0, a1, a2, a3, a4, a5)
}

#[inline(always)]
pub(crate) unsafe fn wait(ep: u64, syscall: isize) -> (u64, u64, u64, u64, u64, u64) {
    let mut a0 = ep;
    let mut a1 = 0u64;
    let mut a2 = 0u64;
    let mut a3 = 0u64;
    let mut a4 = 0u64;
    let mut a5 = 0u64;
    unsafe {
        asm!(
            "ecall",
            inlateout("a0") a0,
            inlateout("a1") a1,
            inlateout("a2") a2,
            inlateout("a3") a3,
            inlateout("a4") a4,
            inlateout("a5") a5,
            inlateout("a7") syscall => _,
            clobber_abi("C"),
            options(nostack)
        );
    }
    (a0, a1, a2, a3, a4, a5)
}

#[inline(always)]
pub(crate) unsafe fn reply_recv_with_reply(
    ep: u64,
    info: u64,
    mr0: u64,
    mr1: u64,
    mr2: u64,
    mr3: u64,
    reply: u64,
) -> (u64, u64, u64, u64, u64, u64) {
    let mut a0 = ep;
    let mut a1 = info;
    let mut a2 = mr0;
    let mut a3 = mr1;
    let mut a4 = mr2;
    let mut a5 = mr3;
    unsafe {
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
    }
    (a0, a1, a2, a3, a4, a5)
}

#[inline(always)]
pub(crate) unsafe fn send(dest: u64, info: u64, mr0: u64, mr1: u64, mr2: u64, mr3: u64) {
    unsafe {
        asm!(
            "ecall",
            inlateout("a0") dest => _,
            inlateout("a1") info => _,
            inlateout("a2") mr0 => _,
            inlateout("a3") mr1 => _,
            inlateout("a4") mr2 => _,
            inlateout("a5") mr3 => _,
            inlateout("a7") SYS_SEND => _,
            clobber_abi("C"),
            options(nostack)
        );
    }
}

#[inline(always)]
pub(crate) unsafe fn yield_now() {
    unsafe {
        asm!(
            "ecall",
            inlateout("a7") SYS_YIELD => _,
            clobber_abi("C"),
            options(nostack)
        );
    }
}

#[inline(always)]
pub(crate) unsafe fn debug_put_char(ch: u8) {
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

#[inline(always)]
pub(crate) unsafe fn debug_halt() {
    unsafe {
        asm!(
            "ecall",
            inlateout("a7") SYS_DEBUG_HALT => _,
            clobber_abi("C"),
            options(nostack)
        );
    }
}
