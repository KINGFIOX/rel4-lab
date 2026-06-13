pub(crate) type FrameMap = (u64, u64, bool, bool);

#[cfg(target_arch = "riscv64")]
mod mmio;
#[cfg(target_arch = "riscv64")]
pub(crate) use mmio::*;

#[cfg(target_arch = "loongarch64")]
mod pci;
#[cfg(target_arch = "loongarch64")]
pub(crate) use pci::*;

#[cfg(not(any(target_arch = "riscv64", target_arch = "loongarch64")))]
compile_error!("unsupported xv6-host disk transport target architecture");
