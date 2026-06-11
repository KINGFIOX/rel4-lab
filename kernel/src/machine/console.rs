//! UART-backed console routing for QEMU `virt`.
//!
//! OpenSBI v0.9 lacks the SBI Debug Console extension and the legacy SBI
//! console is intentionally unused. Rust kernel prints and seL4 debug
//! syscalls use the QEMU `pci-serial` debug UART. User-visible xv6 console
//! I/O is owned by the userspace uart-server on QEMU's default serial0.

use core::ptr::{read_volatile, write_volatile};

use crate::arch::riscv64::vspace::paddr_to_pptr;

const PCI_ECAM_BASE_PA: usize = 0x3000_0000;
const PCI_IO_BASE_PA: usize = 0x0300_0000;
const PCI_DEBUG_UART_PORT: usize = 0x1000;
const PCI_QEMU_VENDOR_ID: u16 = 0x1b36;
const PCI_SERIAL_DEVICE_ID: u16 = 0x0002;
const PCI_CLASS_SERIAL: u16 = 0x0700;
const PCI_COMMAND_IO: u16 = 1 << 0;
const PCI_BAR_IO_SPACE: u32 = 1 << 0;

#[repr(usize)]
#[derive(Copy, Clone, Debug, Eq, PartialEq)]
enum UartRegister {
    Data = 0,
    InterruptEnable = 1,
    FifoControl = 2,
    LineControl = 3,
    LineStatus = 5,
}

impl UartRegister {
    const fn offset(self) -> usize {
        self as usize
    }
}

const LSR_DR: u8 = 1 << 0;
const LSR_THRE: u8 = 1 << 5;
const UART_WAIT_SPINS: usize = 1_000_000;

/// Called once the kernel has switched to a page table with the PSpace
/// direct map. Before this point PA-based UART MMIO addresses are not safe.
pub fn init() {
    let _ = init_pci_debug_uart();
    crate::logger::init();
}

pub fn putc(c: u8) {
    let _ = uart_try_putc(pci_debug_uart_base_pa(), c);
}

fn init_pci_debug_uart() -> bool {
    let Some(cfg) = find_pci_debug_uart_config() else {
        return false;
    };

    write32(cfg, 0x10, (PCI_DEBUG_UART_PORT as u32) | PCI_BAR_IO_SPACE);
    write16(cfg, 0x04, PCI_COMMAND_IO);
    init_16550(pci_debug_uart_base_pa(), true);
    true
}

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

#[inline]
fn pci_config_base(bus: usize, device: usize, function: usize) -> usize {
    PCI_ECAM_BASE_PA + (bus << 20) + (device << 15) + (function << 12)
}

#[inline]
fn pci_debug_uart_base_pa() -> usize {
    PCI_IO_BASE_PA + PCI_DEBUG_UART_PORT
}

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

#[inline]
fn uart_reg(base_pa: usize, register: UartRegister) -> *mut u8 {
    paddr_to_pptr(base_pa + register.offset()) as *mut u8
}

#[inline]
fn pci_reg<T>(cfg_base_pa: usize, offset: usize) -> *mut T {
    paddr_to_pptr(cfg_base_pa + offset) as *mut T
}

#[inline]
fn read16(cfg_base_pa: usize, offset: usize) -> u16 {
    unsafe { read_volatile(pci_reg(cfg_base_pa, offset)) }
}

#[inline]
fn write16(cfg_base_pa: usize, offset: usize, value: u16) {
    unsafe { write_volatile(pci_reg(cfg_base_pa, offset), value) }
}

#[inline]
fn write32(cfg_base_pa: usize, offset: usize, value: u32) {
    unsafe { write_volatile(pci_reg(cfg_base_pa, offset), value) }
}
