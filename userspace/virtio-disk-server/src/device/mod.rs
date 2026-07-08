#[cfg(target_arch = "riscv64")]
mod mmio;
#[cfg(target_arch = "riscv64")]
pub use mmio::*;

#[cfg(target_arch = "loongarch64")]
mod pci;
#[cfg(target_arch = "loongarch64")]
pub use pci::*;

#[cfg(not(any(target_arch = "riscv64", target_arch = "loongarch64")))]
compile_error!("unsupported virtio-disk-server target architecture");
