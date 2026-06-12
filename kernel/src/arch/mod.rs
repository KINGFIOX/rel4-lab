#[cfg(target_arch = "riscv64")]
pub mod riscv64;

#[cfg(target_arch = "loongarch64")]
pub mod loongarch64;

#[cfg(not(any(target_arch = "riscv64", target_arch = "loongarch64")))]
compile_error!("unsupported kernel target architecture");
