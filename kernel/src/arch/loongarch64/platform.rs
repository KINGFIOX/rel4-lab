//! QEMU `virt` platform constants for the LoongArch64 kernel backend.

pub const UART0_MMIO_BASE_PA: usize = 0x1fe0_01e0;
pub const UART0_MMIO_SIZE: usize = 0x100;
pub const UART0_MMIO_FRAME_BASE_PA: usize = 0x1fe0_0000;

pub const PCI_ECAM_BASE_PA: usize = 0x2000_0000;
// DTB maps PCI I/O child address 0x4000 to CPU PA 0x1800_4000.
pub const PCI_IO_BASE_PA: usize = 0x1800_0000;
pub const PCI_DEBUG_UART_PORT: usize = 0x4000;
pub const PCI_IO_SIZE: usize = 0x0000_c000;
pub const PCI_MEM_BASE_PA: usize = 0x4000_0000;
pub const PCI_MEM_SIZE: usize = 0x4000_0000;

pub const PCH_PIC_BASE_PA: usize = 0x1000_0000;
pub const PCH_MSI_BASE_PA: usize = 0x2ff0_0000;

pub const FREE_RAM_REGIONS: &[(u64, u64)] = &[
    // QEMU loongarch64 virt -m 3072 exposes high RAM at
    // [0x8000_0000, 0x1_3000_0000); keep the first 32 MiB clear for the
    // kernel/rootserver/loader staging path.
    (0x8200_0000, 0x1_3000_0000),
];

pub const DEVICE_UNTYPED_REGIONS: &[(u64, u64)] = &[
    // Keep the low memory bank [0, 0x1000_0000) reserved for boot/kernel
    // staging, but expose the UART page that the userspace stack retypes and
    // maps explicitly.
    (
        UART0_MMIO_FRAME_BASE_PA as u64,
        (UART0_MMIO_FRAME_BASE_PA + 0x1000) as u64,
    ),
    // The rest of the user-visible QEMU virt MMIO range starts at the PCH PIC.
    (0x1000_0000, 0x8000_0000),
];
