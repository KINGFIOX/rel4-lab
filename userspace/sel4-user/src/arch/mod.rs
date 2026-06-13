#[cfg(target_arch = "loongarch64")]
pub(crate) mod loongarch64;
#[cfg(target_arch = "loongarch64")]
pub(crate) use loongarch64 as current;

#[cfg(target_arch = "riscv64")]
pub(crate) mod riscv64;
#[cfg(target_arch = "riscv64")]
pub(crate) use riscv64 as current;

#[cfg(not(any(target_arch = "riscv64", target_arch = "loongarch64")))]
compile_error!("unsupported sel4-user target architecture");
