#![no_std]
#![no_main]
#![allow(dead_code)]

//! Rust seL4-compatible microkernel.
//!
//! Architecture backends live under `arch/{riscv64,x86_64}` and are selected
//! at compile time through `arch::current`. RISC-V is the runnable target;
//! x86_64 currently compiles and links with trap/timer stubs.

extern crate core;

#[macro_use]
#[cfg(any(target_arch = "riscv64", target_arch = "x86_64"))]
mod print;

#[cfg(any(target_arch = "riscv64", target_arch = "x86_64"))]
mod abi;
#[cfg(any(target_arch = "riscv64", target_arch = "x86_64"))]
mod api;
#[cfg(any(target_arch = "riscv64", target_arch = "x86_64"))]
mod arch;
#[cfg(any(target_arch = "riscv64", target_arch = "x86_64"))]
mod kernel;
#[cfg(any(target_arch = "riscv64", target_arch = "x86_64"))]
mod logger;
#[cfg(any(target_arch = "riscv64", target_arch = "x86_64"))]
mod machine;
#[cfg(any(target_arch = "riscv64", target_arch = "x86_64"))]
mod object;

#[cfg(any(target_arch = "riscv64", target_arch = "x86_64"))]
pub use arch::current::kernel::boot::_start;
#[cfg(any(target_arch = "riscv64", target_arch = "x86_64"))]
pub use arch::current::kernel::boot::init_kernel;
#[cfg(any(target_arch = "riscv64", target_arch = "x86_64"))]
pub use log_crate::{debug, error, info, trace, warn};

/// Panic handler — print location + message, then halt.
#[cfg(any(target_arch = "riscv64", target_arch = "x86_64"))]
#[panic_handler]
fn panic(info: &core::panic::PanicInfo) -> ! {
    println!("\n*** KERNEL PANIC ***");
    if let Some(loc) = info.location() {
        println!("at {}:{}", loc.file(), loc.line());
    }
    println!("{}", info.message());
    arch::current::kernel::boot::halt()
}
