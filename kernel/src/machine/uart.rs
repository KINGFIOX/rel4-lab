//! NS16550A UART driver for QEMU `virt` (MMIO at 0x1000_0000).
//!
//! This driver is intentionally tiny and synchronous; it is only used for
//! boot-time / panic messages. Once we switch to S-mode (M2+) we will go
//! through SBI `console_putchar` instead.

use core::ptr::{read_volatile, write_volatile};

const UART_BASE: usize = 0x1000_0000;

#[allow(dead_code)]
const RHR: usize = 0; // Receive holding register (read)
const THR: usize = 0; // Transmit holding register (write)
const IER: usize = 1; // Interrupt enable register
const FCR: usize = 2; // FIFO control register (write)
const LCR: usize = 3; // Line control register
const LSR: usize = 5; // Line status register

const LSR_THRE: u8 = 1 << 5; // Transmitter holding register empty

#[inline(always)]
fn reg(off: usize) -> *mut u8 {
    (UART_BASE + off) as *mut u8
}

pub fn init() {
    unsafe {
        // Disable all interrupts.
        write_volatile(reg(IER), 0x00);
        // Enable FIFO, clear TX/RX FIFOs, 14-byte threshold.
        write_volatile(reg(FCR), 0x07);
        // 8 bits, no parity, 1 stop bit. (QEMU ignores baud divisor, skip it.)
        write_volatile(reg(LCR), 0x03);
    }
}

#[inline]
pub fn putc(c: u8) {
    unsafe {
        while read_volatile(reg(LSR)) & LSR_THRE == 0 {}
        write_volatile(reg(THR), c);
    }
}

pub fn write_str(s: &str) {
    for &b in s.as_bytes() {
        if b == b'\n' {
            putc(b'\r');
        }
        putc(b);
    }
}

pub fn write_dec(mut n: u64) {
    if n == 0 {
        putc(b'0');
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
        putc(buf[i]);
    }
}

#[allow(dead_code)]
pub fn write_hex(n: u64) {
    putc(b'0');
    putc(b'x');
    let mut started = false;
    for i in (0..16).rev() {
        let nib = ((n >> (i * 4)) & 0xF) as u8;
        if nib != 0 || started || i == 0 {
            started = true;
            putc(if nib < 10 {
                b'0' + nib
            } else {
                b'a' + (nib - 10)
            });
        }
    }
}
