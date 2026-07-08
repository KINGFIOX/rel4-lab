#[cfg(target_arch = "riscv64")]
pub mod riscv64;
#[cfg(target_arch = "riscv64")]
pub use riscv64 as current;

#[cfg(target_arch = "loongarch64")]
pub mod loongarch64;
#[cfg(target_arch = "loongarch64")]
#[allow(unused_imports)]
pub use loongarch64 as current;

#[cfg(target_arch = "x86_64")]
pub mod x86_64;
#[cfg(target_arch = "x86_64")]
pub use x86_64 as current;

#[cfg(not(any(
    target_arch = "riscv64",
    target_arch = "loongarch64",
    target_arch = "x86_64"
)))]
compile_error!("unsupported kernel target architecture");
