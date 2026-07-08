use super::{MMIO_FRAME_SIZE, UartMmio, VirtioMmio, VirtioTransport};

pub const UART0_MMIO_BASE: u64 = 0x1000_0000;
pub const UART0_MMIO_SIZE: u64 = 0x1000;
pub const UART0_MMIO_FRAME_BASE: u64 = UART0_MMIO_BASE;
pub const XV6_UART_MMIO_FRAME_VADDR: u64 = 0x5000_4000;
pub const XV6_UART_MMIO_VADDR: u64 = XV6_UART_MMIO_FRAME_VADDR;
pub const UART0_IRQ: u64 = 10;

pub const VIRTIO_MMIO_BASE: u64 = 0x1000_1000;
pub const VIRTIO_MMIO_SIZE: u64 = 0x1000;
pub const VIRTIO_MMIO_FRAME_BASE: u64 = VIRTIO_MMIO_BASE;
pub const XV6_VIRTIO_MMIO_FRAME_VADDR: u64 = 0x5000_0000;
pub const XV6_VIRTIO_MMIO_VADDR: u64 = XV6_VIRTIO_MMIO_FRAME_VADDR;
pub const VIRTIO0_IRQ: u64 = 1;

pub const XV6_DEVICE_MMIO_BASE: u64 = UART0_MMIO_FRAME_BASE;
pub const XV6_DEVICE_MMIO_SIZE: u64 =
    VIRTIO_MMIO_FRAME_BASE + MMIO_FRAME_SIZE - XV6_DEVICE_MMIO_BASE;

pub const UART0: UartMmio = UartMmio {
    paddr: UART0_MMIO_BASE,
    size: UART0_MMIO_SIZE,
    frame_paddr: UART0_MMIO_FRAME_BASE,
    frame_vaddr: XV6_UART_MMIO_FRAME_VADDR,
    vaddr: XV6_UART_MMIO_VADDR,
    irq: UART0_IRQ,
};

pub const VIRTIO0_MMIO: VirtioMmio = VirtioMmio {
    paddr: VIRTIO_MMIO_BASE,
    size: VIRTIO_MMIO_SIZE,
    frame_paddr: VIRTIO_MMIO_FRAME_BASE,
    frame_vaddr: XV6_VIRTIO_MMIO_FRAME_VADDR,
    vaddr: XV6_VIRTIO_MMIO_VADDR,
    irq: VIRTIO0_IRQ,
};

pub const VIRTIO_TRANSPORT: VirtioTransport = VirtioTransport::Mmio(VIRTIO0_MMIO);
