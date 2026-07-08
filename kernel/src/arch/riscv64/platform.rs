pub const PCI_ECAM_BASE_PA: usize = 0x3000_0000;
pub const PCI_IO_BASE_PA: usize = 0x0300_0000;
pub const PCI_DEBUG_UART_PORT: usize = 0x1000;

pub const FREE_RAM_REGIONS: &[(u64, u64)] = &[
    // 2 MiB aligned, after the rootserver and elfloader staging area.
    (0x8200_0000, 0x1_4000_0000),
];

pub const DEVICE_UNTYPED_REGIONS: &[(u64, u64)] = &[
    // QEMU virt MMIO lives below the DRAM base.
    (0x0, 0x8000_0000),
];
