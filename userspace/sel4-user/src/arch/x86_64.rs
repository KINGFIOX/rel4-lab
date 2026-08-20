use core::arch::asm;

use crate::{
    SYS_CALL, SYS_DEBUG_HALT, SYS_DEBUG_PUT_CHAR, SYS_REPLY, SYS_REPLY_RECV, SYS_SEND,
    SYS_SET_TLS_BASE, SYS_YIELD, ThreadCtl,
};

pub(crate) const KERNEL_TIMER_IRQ: u64 = 256;

#[inline(always)]
pub(crate) unsafe fn call(
    service: u64,
    info: u64,
    mr0: u64,
    mr1: u64,
    mr2: u64,
    mr3: u64,
) -> (u64, u64, u64, u64, u64, u64) {
    let mut rdi = service;
    let mut rsi = info;
    let mut r10 = mr0;
    let mut r8 = mr1;
    let mut r9 = mr2;
    let mut r15 = mr3;
    unsafe {
        asm!(
            "syscall",
            inlateout("rdi") rdi,
            inlateout("rsi") rsi,
            inlateout("r10") r10,
            inlateout("r8") r8,
            inlateout("r9") r9,
            inlateout("r15") r15,
            inlateout("rax") SYS_CALL => _,
            out("rcx") _,
            out("r11") _,
            clobber_abi("C"),
            options(nostack)
        );
    }
    (rdi, rsi, r10, r8, r9, r15)
}

#[inline(always)]
#[allow(dead_code)]
pub(crate) unsafe fn recv_with_reply(
    ep: u64,
    reply: u64,
    syscall: isize,
) -> (u64, u64, u64, u64, u64, u64) {
    let mut rdi = ep;
    let mut rsi = 0u64;
    let mut r10 = 0u64;
    let mut r8 = 0u64;
    let mut r9 = 0u64;
    let mut r15 = 0u64;
    unsafe {
        asm!(
            "syscall",
            inlateout("rdi") rdi,
            inlateout("rsi") rsi,
            inlateout("r10") r10,
            inlateout("r8") r8,
            inlateout("r9") r9,
            inlateout("r15") r15,
            inlateout("r12") reply => _,
            inlateout("rax") syscall => _,
            out("rcx") _,
            out("r11") _,
            clobber_abi("C"),
            options(nostack)
        );
    }
    (rdi, rsi, r10, r8, r9, r15)
}

#[inline(always)]
pub(crate) unsafe fn wait(ep: u64, syscall: isize) -> (u64, u64, u64, u64, u64, u64) {
    let mut rdi = ep;
    let mut rsi = 0u64;
    let mut r10 = 0u64;
    let mut r8 = 0u64;
    let mut r9 = 0u64;
    let mut r15 = 0u64;
    unsafe {
        asm!(
            "syscall",
            inlateout("rdi") rdi,
            inlateout("rsi") rsi,
            inlateout("r10") r10,
            inlateout("r8") r8,
            inlateout("r9") r9,
            inlateout("r15") r15,
            inlateout("rax") syscall => _,
            out("rcx") _,
            out("r11") _,
            clobber_abi("C"),
            options(nostack)
        );
    }
    (rdi, rsi, r10, r8, r9, r15)
}

#[inline(always)]
pub(crate) unsafe fn reply_recv(
    ep: u64,
    info: u64,
    mr0: u64,
    mr1: u64,
    mr2: u64,
    mr3: u64,
) -> (u64, u64, u64, u64, u64, u64) {
    let mut rdi = ep;
    let mut rsi = info;
    let mut r10 = mr0;
    let mut r8 = mr1;
    let mut r9 = mr2;
    let mut r15 = mr3;
    unsafe {
        asm!(
            "syscall",
            inlateout("rdi") rdi,
            inlateout("rsi") rsi,
            inlateout("r10") r10,
            inlateout("r8") r8,
            inlateout("r9") r9,
            inlateout("r15") r15,
            inlateout("rax") SYS_REPLY_RECV => _,
            out("rcx") _,
            out("r11") _,
            clobber_abi("C"),
            options(nostack)
        );
    }
    (rdi, rsi, r10, r8, r9, r15)
}

#[inline(always)]
#[allow(dead_code)]
pub(crate) unsafe fn reply_recv_with_reply(
    ep: u64,
    info: u64,
    mr0: u64,
    mr1: u64,
    mr2: u64,
    mr3: u64,
    reply: u64,
) -> (u64, u64, u64, u64, u64, u64) {
    let mut rdi = ep;
    let mut rsi = info;
    let mut r10 = mr0;
    let mut r8 = mr1;
    let mut r9 = mr2;
    let mut r15 = mr3;
    unsafe {
        asm!(
            "syscall",
            inlateout("rdi") rdi,
            inlateout("rsi") rsi,
            inlateout("r10") r10,
            inlateout("r8") r8,
            inlateout("r9") r9,
            inlateout("r15") r15,
            inlateout("r12") reply => _,
            inlateout("rax") SYS_REPLY_RECV => _,
            out("rcx") _,
            out("r11") _,
            clobber_abi("C"),
            options(nostack)
        );
    }
    (rdi, rsi, r10, r8, r9, r15)
}

#[inline(always)]
pub(crate) unsafe fn send(dest: u64, info: u64, mr0: u64, mr1: u64, mr2: u64, mr3: u64) {
    unsafe {
        asm!(
            "syscall",
            inlateout("rdi") dest => _,
            inlateout("rsi") info => _,
            inlateout("r10") mr0 => _,
            inlateout("r8") mr1 => _,
            inlateout("r9") mr2 => _,
            inlateout("r15") mr3 => _,
            inlateout("rax") SYS_SEND => _,
            out("rcx") _,
            out("r11") _,
            clobber_abi("C"),
            options(nostack)
        );
    }
}

#[inline(always)]
pub(crate) unsafe fn reply(info: u64, mr0: u64, mr1: u64, mr2: u64, mr3: u64) {
    unsafe {
        asm!(
            "syscall",
            inlateout("rsi") info => _,
            inlateout("r10") mr0 => _,
            inlateout("r8") mr1 => _,
            inlateout("r9") mr2 => _,
            inlateout("r15") mr3 => _,
            inlateout("rax") SYS_REPLY => _,
            out("rcx") _,
            out("r11") _,
            clobber_abi("C"),
            options(nostack)
        );
    }
}

#[inline(always)]
pub(crate) unsafe fn yield_now() {
    unsafe {
        asm!(
            "syscall",
            inlateout("rax") SYS_YIELD => _,
            out("rcx") _,
            out("r11") _,
            clobber_abi("C"),
            options(nostack)
        );
    }
}

#[inline(always)]
pub(crate) unsafe fn debug_put_char(ch: u8) {
    unsafe {
        asm!(
            "syscall",
            inlateout("rdi") ch as u64 => _,
            inlateout("rax") SYS_DEBUG_PUT_CHAR => _,
            out("rcx") _,
            out("r11") _,
            clobber_abi("C"),
            options(nostack)
        );
    }
}

#[inline(always)]
pub(crate) unsafe fn debug_halt() {
    unsafe {
        asm!(
            "syscall",
            inlateout("rax") SYS_DEBUG_HALT => _,
            out("rcx") _,
            out("r11") _,
            clobber_abi("C"),
            options(nostack)
        );
    }
}

#[inline(always)]
pub(crate) unsafe fn set_tls_base(tls_base: u64) {
    unsafe {
        asm!(
            "syscall",
            inlateout("rdi") tls_base => _,
            inlateout("rax") SYS_SET_TLS_BASE => _,
            out("rcx") _,
            out("r11") _,
            clobber_abi("C"),
            options(nostack)
        );
    }
}

#[inline(always)]
pub(crate) unsafe fn thread_ctl() -> *mut ThreadCtl {
    let ptr: u64;
    unsafe {
        asm!(
            "mov {}, qword ptr fs:[0]",
            out(reg) ptr,
            options(nostack, readonly, preserves_flags)
        );
    }
    ptr as *mut ThreadCtl
}
