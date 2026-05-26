//! Boot-time console abstraction.
//!
//! - In M-mode (M1 standalone path), we talk to the QEMU virt NS16550 UART
//!   directly via MMIO.
//! - In S-mode (M2+ under elfloader), the UART is owned by SBI/firmware and
//!   we go through `sbi_console_putchar` (legacy ext 0x01).
//!
//! At compile time we pick S-mode (SBI) by default. M-mode UART path is
//! kept available for the standalone M1 simulate target.

use crate::arch::riscv64::sbi;

pub fn putc(c: u8) {
    sbi::console_putchar(c);
}
