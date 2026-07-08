pub const PCI_ECAM_BASE_PA: usize = 0xe000_0000;
pub const PCI_IO_BASE_PA: usize = 0;
pub const PCI_DEBUG_UART_PORT: usize = 0x3f8;

pub const FREE_RAM_REGIONS: &[(u64, u64)] = &[(0x0020_0000, 0x8000_0000)];

pub const DEVICE_UNTYPED_REGIONS: &[(u64, u64)] =
    &[(0x0000_0000, 0x0020_0000), (0x8000_0000, 0x1_0000_0000)];
