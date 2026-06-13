//! QEMU `virt` platform constants for the LoongArch64 kernel backend.

pub const UART0_MMIO_BASE_PA: usize = 0x1fe0_01e0;
pub const UART0_MMIO_SIZE: usize = 0x100;

pub const PCI_ECAM_BASE_PA: usize = 0x2000_0000;
// DTB maps PCI I/O child address 0x4000 to CPU PA 0x1800_4000.
pub const PCI_IO_BASE_PA: usize = 0x1800_0000;
pub const PCI_DEBUG_UART_PORT: usize = 0x4000;
pub const PCI_IO_SIZE: usize = 0x0000_c000;
pub const PCI_MEM_BASE_PA: usize = 0x4000_0000;
pub const PCI_MEM_SIZE: usize = 0x4000_0000;

pub const PCH_PIC_BASE_PA: usize = 0x1000_0000;
pub const PCH_MSI_BASE_PA: usize = 0x2ff0_0000;
