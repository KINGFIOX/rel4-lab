#![no_std]
#![no_main]
#![allow(dead_code)]

//! Rust seL4-compatible microkernel.
//!
//! Boot flow on `qemu-riscv-virt` (RV64, Sv39):
//!
//! ```text
//! QEMU -> elfloader (M/S-mode) -> kernel `_start` (S-mode, paging on)
//!                                  |
//!                                  v
//!                          init_kernel(a0..a7)
//! ```

extern crate core;

#[cfg(target_arch = "loongarch64")]
compile_error!(
    "LoongArch64 kernel backend is not implemented yet; add the seL4 LoongArch64 ABI/elfloader port before building this target"
);

#[cfg(target_arch = "loongarch64")]
#[panic_handler]
fn loongarch64_unimplemented_panic(_: &core::panic::PanicInfo) -> ! {
    loop {}
}

#[macro_use]
#[cfg(target_arch = "riscv64")]
mod print;

#[cfg(target_arch = "riscv64")]
mod abi;
#[cfg(target_arch = "riscv64")]
mod api;
#[cfg(target_arch = "riscv64")]
mod arch;
#[cfg(target_arch = "riscv64")]
mod kernel;
#[cfg(target_arch = "riscv64")]
mod logger;
#[cfg(target_arch = "riscv64")]
mod machine;
#[cfg(target_arch = "riscv64")]
mod object;

#[cfg(target_arch = "riscv64")]
pub use arch::riscv64::boot::_start;
#[cfg(target_arch = "riscv64")]
pub use arch::riscv64::boot::init_kernel;
#[cfg(target_arch = "riscv64")]
pub use log_crate::{debug, error, info, trace, warn};

/// Panic handler — print location + message, then halt.
#[cfg(target_arch = "riscv64")]
#[panic_handler]
fn panic(info: &core::panic::PanicInfo) -> ! {
    println!("\n*** KERNEL PANIC ***");
    if let Some(loc) = info.location() {
        println!("at {}:{}", loc.file(), loc.line());
    }
    println!("{}", info.message());
    arch::riscv64::boot::halt()
}
