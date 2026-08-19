#[cfg(target_arch = "riscv64")]
pub(crate) mod riscv64;
#[cfg(target_arch = "riscv64")]
pub(crate) use riscv64 as current;

#[cfg(target_arch = "x86_64")]
pub(crate) mod x86_64;
#[cfg(target_arch = "x86_64")]
pub(crate) use x86_64 as current;

#[cfg(not(any(target_arch = "riscv64", target_arch = "x86_64")))]
compile_error!("unsupported sel4-user target architecture");
