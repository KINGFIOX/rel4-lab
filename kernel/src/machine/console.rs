//! UART-backed console routing.
//!
//! RISC-V QEMU `virt` uses the `pci-serial` debug UART. x86 QEMU `pc` uses
//! COM1 at I/O port `0x3f8` (`plat::PCI_DEBUG_UART_PORT`).

use crate::arch::current::plat as platform;

#[cfg(target_arch = "riscv64")]
use crate::arch::current::object::vspace::paddr_to_mmio;
#[cfg(target_arch = "riscv64")]
use crate::ktypes::addr::Paddr;
#[cfg(target_arch = "riscv64")]
use crate::ktypes::mmio::{MmioReg, MmioRegion, MmioValue};

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
    // SAFETY: these are COM1's own I/O ports, written with the standard
    // 16550 "interrupts off, FIFOs on, 8N1" setup.
    unsafe {
        outb(port + UartRegister::InterruptEnable.offset(), 0x00);
        outb(port + UartRegister::FifoControl.offset(), 0x07);
        outb(port + UartRegister::LineControl.offset(), 0x03);
    }
}

#[cfg(target_arch = "x86_64")]
fn com1_try_putc(ch: u8) -> bool {
    let port = platform::PCI_DEBUG_UART_PORT as u16;
    // SAFETY: reading COM1's line status and writing its data register.
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

/// Write a byte to an I/O port.
///
/// # Safety
/// `port` must be a port the caller is allowed to drive, since the write goes
/// straight to whatever device answers it.
#[cfg(target_arch = "x86_64")]
#[inline]
unsafe fn outb(port: u16, value: u8) {
    // SAFETY: forwarded to the caller.
    unsafe {
        asm!(
            "out dx, al",
            in("dx") port,
            in("al") value,
            options(nomem, nostack, preserves_flags),
        );
    }
}

/// Read a byte from an I/O port.
///
/// # Safety
/// `port` must be a port the caller is allowed to drive; reads can have side
/// effects on the device.
#[cfg(target_arch = "x86_64")]
#[inline]
unsafe fn inb(port: u16) -> u8 {
    let value: u8;
    // SAFETY: forwarded to the caller.
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
    uart_reg(base_pa, UartRegister::InterruptEnable).write(0x00);
    uart_reg(base_pa, UartRegister::FifoControl).write(if clear_fifos { 0x07 } else { 0x01 });
    uart_reg(base_pa, UartRegister::LineControl).write(0x03);
}

#[cfg(target_arch = "riscv64")]
fn uart_try_putc(base_pa: usize, ch: u8) -> bool {
    for _ in 0..UART_WAIT_SPINS {
        if uart_reg(base_pa, UartRegister::LineStatus).read() & LSR_THRE != 0 {
            uart_reg(base_pa, UartRegister::Data).write(ch);
            return true;
        }
    }
    false
}

/// Device window for a 16550's eight byte-wide registers.
#[cfg(target_arch = "riscv64")]
fn uart_window(base_pa: usize) -> MmioRegion {
    // SAFETY: the platform's UART sits at this physical address and the
    // kernel's MMIO window maps it; no Rust object lives there.
    unsafe { MmioRegion::new(paddr_to_mmio(Paddr::new(base_pa)), 8) }
}

#[cfg(target_arch = "riscv64")]
#[inline]
fn uart_reg(base_pa: usize, register: UartRegister) -> MmioReg<u8> {
    uart_window(base_pa).reg(register.offset() as usize)
}

/// One device's 4 KiB PCI configuration space.
#[cfg(target_arch = "riscv64")]
fn pci_config_window(cfg_base_pa: usize) -> MmioRegion {
    // SAFETY: `cfg_base_pa` comes from `pci_config_base`, which addresses one
    // function's 4 KiB slot inside the platform's ECAM region.
    unsafe { MmioRegion::new(paddr_to_mmio(Paddr::new(cfg_base_pa)), 0x1000) }
}

#[cfg(target_arch = "riscv64")]
#[inline]
fn pci_reg<T: MmioValue>(cfg_base_pa: usize, offset: usize) -> MmioReg<T> {
    pci_config_window(cfg_base_pa).reg(offset)
}

#[cfg(target_arch = "riscv64")]
#[inline]
fn read16(cfg_base_pa: usize, offset: usize) -> u16 {
    pci_reg::<u16>(cfg_base_pa, offset).read()
}

#[cfg(target_arch = "riscv64")]
#[inline]
fn write16(cfg_base_pa: usize, offset: usize, value: u16) {
    pci_reg::<u16>(cfg_base_pa, offset).write(value);
}

#[cfg(target_arch = "riscv64")]
#[inline]
fn write32(cfg_base_pa: usize, offset: usize, value: u32) {
    pci_reg::<u32>(cfg_base_pa, offset).write(value);
}
