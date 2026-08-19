#[cfg(target_arch = "riscv64")]
pub mod riscv64;
#[cfg(target_arch = "riscv64")]
pub use riscv64 as current;

#[cfg(not(target_arch = "riscv64"))]
compile_error!("unsupported linux-compat platform target architecture");

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
