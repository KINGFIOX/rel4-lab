#[cfg(target_arch = "riscv64")]
pub(crate) mod riscv64;
#[cfg(target_arch = "riscv64")]
pub(crate) use riscv64 as current;

#[cfg(not(target_arch = "riscv64"))]
compile_error!("unsupported sel4-user target architecture");
