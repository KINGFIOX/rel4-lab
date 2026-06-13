#[cfg(target_arch = "loongarch64")]
pub mod loongarch64;
#[cfg(target_arch = "loongarch64")]
pub use loongarch64 as current;

#[cfg(target_arch = "riscv64")]
pub mod riscv64;
#[cfg(target_arch = "riscv64")]
pub use riscv64 as current;

#[cfg(not(any(target_arch = "riscv64", target_arch = "loongarch64")))]
compile_error!("unsupported xv6 platform target architecture");

pub const MMIO_FRAME_SIZE: u64 = 0x1000;

#[derive(Copy, Clone, Eq, PartialEq)]
pub struct UartMmio {
    pub paddr: u64,
    pub size: u64,
    pub frame_paddr: u64,
    pub frame_vaddr: u64,
    pub vaddr: u64,
    pub irq: u64,
}

#[derive(Copy, Clone, Eq, PartialEq)]
pub struct VirtioMmio {
    pub paddr: u64,
    pub size: u64,
    pub frame_paddr: u64,
    pub frame_vaddr: u64,
    pub vaddr: u64,
    pub irq: u64,
}

#[derive(Copy, Clone, Eq, PartialEq)]
pub struct PciHost {
    pub ecam_paddr: u64,
    pub ecam_size: u64,
    pub io_paddr: u64,
    pub io_size: u64,
    pub mem_paddr: u64,
    pub mem_size: u64,
    pub legacy_irq_base: u64,
    pub legacy_irq_count: u64,
    pub msi_paddr: u64,
    pub msi_size: u64,
    pub msi_base_vector: u64,
    pub msi_num_vectors: u64,
}

#[derive(Copy, Clone, Eq, PartialEq)]
pub enum VirtioTransport {
    Mmio(VirtioMmio),
    Pci(PciHost),
}
