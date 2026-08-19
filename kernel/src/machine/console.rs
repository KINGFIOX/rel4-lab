//! UART-backed console routing.
//!
//! RISC-V QEMU `virt` uses the `pci-serial` debug UART. x86 QEMU `pc` uses
//! COM1 at I/O port `0x3f8` (`plat::PCI_DEBUG_UART_PORT`).

use crate::arch::current::plat as platform;

#[cfg(target_arch = "riscv64")]
use crate::arch::current::object::vspace::paddr_to_mmio;
#[cfg(target_arch = "riscv64")]
use core::ptr::{read_volatile, write_volatile};

#[cfg(target_arch = "x86_64")]
use core::arch::asm;

const LSR_THRE: u8 = 1 << 5;
const UART_WAIT_SPINS: usize = 1024;

#[repr(u16)]
#[derive(Copy, Clone)]
enum UartRegister {
    Data = 0,
    InterruptEnable = 1,
    FifoControl = 2,
    LineControl = 3,
    LineStatus = 5,
}

impl UartRegister {
    const fn offset(self) -> u16 {
        self as u16
    }
}

/// Called once the kernel has installed its address-space window.
pub fn init() {
    #[cfg(target_arch = "riscv64")]
    let _ = init_pci_debug_uart();
    #[cfg(target_arch = "x86_64")]
    init_com1();
    crate::logger::init();
}

pub fn putc(c: u8) {
    #[cfg(target_arch = "riscv64")]
    let _ = uart_try_putc(pci_debug_uart_base_pa(), c);
    #[cfg(target_arch = "x86_64")]
    let _ = com1_try_putc(c);
}

#[cfg(target_arch = "x86_64")]
fn init_com1() {
    let port = platform::PCI_DEBUG_UART_PORT as u16;
    unsafe {
        outb(port + UartRegister::InterruptEnable.offset(), 0x00);
        outb(port + UartRegister::FifoControl.offset(), 0x07);
        outb(port + UartRegister::LineControl.offset(), 0x03);
    }
}

#[cfg(target_arch = "x86_64")]
fn com1_try_putc(ch: u8) -> bool {
    let port = platform::PCI_DEBUG_UART_PORT as u16;
    unsafe {
        for _ in 0..UART_WAIT_SPINS {
            if inb(port + UartRegister::LineStatus.offset()) & LSR_THRE != 0 {
                outb(port + UartRegister::Data.offset(), ch);
                return true;
            }
        }
    }
    false
}

#[cfg(target_arch = "x86_64")]
#[inline]
unsafe fn outb(port: u16, value: u8) {
    unsafe {
        asm!(
            "out dx, al",
            in("dx") port,
            in("al") value,
            options(nomem, nostack, preserves_flags),
        );
    }
}

#[cfg(target_arch = "x86_64")]
#[inline]
unsafe fn inb(port: u16) -> u8 {
    let value: u8;
    unsafe {
        asm!(
            "in al, dx",
            in("dx") port,
            out("al") value,
            options(nomem, nostack, preserves_flags),
        );
    }
    value
}

#[cfg(target_arch = "riscv64")]
const PCI_QEMU_VENDOR_ID: u16 = 0x1b36;
#[cfg(target_arch = "riscv64")]
const PCI_SERIAL_DEVICE_ID: u16 = 0x0002;
#[cfg(target_arch = "riscv64")]
const PCI_CLASS_SERIAL: u16 = 0x0700;
#[cfg(target_arch = "riscv64")]
const PCI_COMMAND_IO: u16 = 1 << 0;
#[cfg(target_arch = "riscv64")]
const PCI_BAR_IO_SPACE: u32 = 1 << 0;

#[cfg(target_arch = "riscv64")]
fn init_pci_debug_uart() -> bool {
    let Some(cfg) = find_pci_debug_uart_config() else {
        return false;
    };

    write32(
        cfg,
        0x10,
        (platform::PCI_DEBUG_UART_PORT as u32) | PCI_BAR_IO_SPACE,
    );
    write16(cfg, 0x04, PCI_COMMAND_IO);
    init_16550(pci_debug_uart_base_pa(), true);
    true
}

#[cfg(target_arch = "riscv64")]
fn find_pci_debug_uart_config() -> Option<usize> {
    let mut device = 0usize;
    while device < 32 {
        let cfg = pci_config_base(0, device, 0);
        let vendor = read16(cfg, 0x00);
        if vendor != 0xffff {
            let device_id = read16(cfg, 0x02);
            let class = read16(cfg, 0x0a);
            if (vendor == PCI_QEMU_VENDOR_ID && device_id == PCI_SERIAL_DEVICE_ID)
                || class == PCI_CLASS_SERIAL
            {
                return Some(cfg);
            }
        }
        device += 1;
    }
    None
}

#[cfg(target_arch = "riscv64")]
#[inline]
fn pci_config_base(bus: usize, device: usize, function: usize) -> usize {
    platform::PCI_ECAM_BASE_PA + (bus << 20) + (device << 15) + (function << 12)
}

#[cfg(target_arch = "riscv64")]
#[inline]
fn pci_debug_uart_base_pa() -> usize {
    platform::PCI_IO_BASE_PA + platform::PCI_DEBUG_UART_PORT
}

#[cfg(target_arch = "riscv64")]
fn init_16550(base_pa: usize, clear_fifos: bool) {
    unsafe {
        write_volatile(uart_reg(base_pa, UartRegister::InterruptEnable), 0x00);
        write_volatile(
            uart_reg(base_pa, UartRegister::FifoControl),
            if clear_fifos { 0x07 } else { 0x01 },
        );
        write_volatile(uart_reg(base_pa, UartRegister::LineControl), 0x03);
    }
}

#[cfg(target_arch = "riscv64")]
fn uart_try_putc(base_pa: usize, ch: u8) -> bool {
    unsafe {
        for _ in 0..UART_WAIT_SPINS {
            if read_volatile(uart_reg(base_pa, UartRegister::LineStatus)) & LSR_THRE != 0 {
                write_volatile(uart_reg(base_pa, UartRegister::Data), ch);
                return true;
            }
        }
    }
    false
}

#[cfg(target_arch = "riscv64")]
#[inline]
fn uart_reg(base_pa: usize, register: UartRegister) -> *mut u8 {
    paddr_to_mmio(base_pa + register.offset() as usize) as *mut u8
}

#[cfg(target_arch = "riscv64")]
#[inline]
fn pci_reg<T>(cfg_base_pa: usize, offset: usize) -> *mut T {
    paddr_to_mmio(cfg_base_pa + offset) as *mut T
}

#[cfg(target_arch = "riscv64")]
#[inline]
fn read16(cfg_base_pa: usize, offset: usize) -> u16 {
    unsafe { read_volatile(pci_reg(cfg_base_pa, offset)) }
}

#[cfg(target_arch = "riscv64")]
#[inline]
fn write16(cfg_base_pa: usize, offset: usize, value: u16) {
    unsafe { write_volatile(pci_reg(cfg_base_pa, offset), value) }
}

#[cfg(target_arch = "riscv64")]
#[inline]
fn write32(cfg_base_pa: usize, offset: usize, value: u32) {
    unsafe { write_volatile(pci_reg(cfg_base_pa, offset), value) }
}
