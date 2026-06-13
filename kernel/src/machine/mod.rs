pub mod console;
#[cfg(target_arch = "loongarch64")]
pub mod loongarch_irq;
#[cfg(target_arch = "riscv64")]
pub mod plic;
