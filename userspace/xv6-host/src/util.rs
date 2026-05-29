use core::arch::asm;

use crate::consts::{PAGE_SIZE, SYS_DEBUG_HALT, SYS_DEBUG_PUT_CHAR};
use crate::sel4::sel4_yield;

pub(crate) fn align_down(v: u64) -> u64 {
    v & !(PAGE_SIZE - 1)
}

pub(crate) fn align_up(v: u64) -> u64 {
    (v + PAGE_SIZE - 1) & !(PAGE_SIZE - 1)
}

pub(crate) fn read_u16(buf: &[u8], off: usize) -> u16 {
    u16::from_le_bytes([buf[off], buf[off + 1]])
}

pub(crate) fn read_u32(buf: &[u8], off: usize) -> u32 {
    u32::from_le_bytes([buf[off], buf[off + 1], buf[off + 2], buf[off + 3]])
}

pub(crate) fn read_u64(buf: &[u8], off: usize) -> u64 {
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

pub(crate) fn write_u16(buf: &mut [u8], off: usize, v: u16) {
    buf[off..off + 2].copy_from_slice(&v.to_le_bytes());
}

pub(crate) fn write_i32(buf: &mut [u8], off: usize, v: i32) {
    buf[off..off + 4].copy_from_slice(&v.to_le_bytes());
}

pub(crate) fn write_u32(buf: &mut [u8], off: usize, v: u32) {
    buf[off..off + 4].copy_from_slice(&v.to_le_bytes());
}

pub(crate) fn write_u64_bytes(buf: &mut [u8], off: usize, v: u64) {
    buf[off..off + 8].copy_from_slice(&v.to_le_bytes());
}

pub(crate) fn putchar(ch: u8) {
    unsafe {
        asm!(
            "ecall",
            in("a0") ch as u64,
            in("a7") SYS_DEBUG_PUT_CHAR,
            options(nostack)
        );
    }
}

pub(crate) fn log(s: &str) {
    for b in s.as_bytes() {
        putchar(*b);
    }
}

pub(crate) fn log_bytes(bytes: &[u8]) {
    for b in bytes {
        putchar(*b);
    }
}

pub(crate) fn print_u64(mut n: u64) {
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

pub(crate) fn print_i64(n: i64) {
    if n < 0 {
        putchar(b'-');
        print_u64(n.wrapping_neg() as u64);
    } else {
        print_u64(n as u64);
    }
}

pub(crate) fn print_hex(mut n: u64) {
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

pub(crate) fn halt_loop() -> ! {
    unsafe {
        asm!("ecall", in("a7") SYS_DEBUG_HALT, options(nostack));
    }
    loop {
        unsafe { sel4_yield() };
    }
}
