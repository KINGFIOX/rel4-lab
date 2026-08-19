use super::UartMmio;

/// QEMU `pci-serial` BAR0 is I/O. uart-server on x86 uses DebugPutChar
/// instead of this window; the constants stay so spawn can skip MMIO.
pub const UART0_MMIO_BASE: u64 = 0;
pub const UART0_MMIO_SIZE: u64 = 0;
pub const UART0_MMIO_FRAME_BASE: u64 = 0;
pub const UART_MMIO_FRAME_VADDR: u64 = 0x5000_4000;
pub const UART_MMIO_VADDR: u64 = UART_MMIO_FRAME_VADDR;
pub const UART0_IRQ: u64 = 0;

pub const DEVICE_MMIO_BASE: u64 = 0;
pub const DEVICE_MMIO_SIZE: u64 = 0;

pub const UART0: UartMmio = UartMmio {
    paddr: UART0_MMIO_BASE,
    size: UART0_MMIO_SIZE,
    frame_paddr: UART0_MMIO_FRAME_BASE,
    frame_vaddr: UART_MMIO_FRAME_VADDR,
    vaddr: UART_MMIO_VADDR,
    irq: UART0_IRQ,
};
