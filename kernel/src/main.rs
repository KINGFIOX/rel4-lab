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

#[macro_use]
mod print;

mod abi;
mod api;
mod arch;
mod kernel;
mod machine;
mod object;
mod xv6_compat;

pub use arch::riscv64::boot::_start;
pub use arch::riscv64::boot::init_kernel;

/// Panic handler — print location + message, then halt.
#[panic_handler]
fn panic(info: &core::panic::PanicInfo) -> ! {
    println!("\n*** KERNEL PANIC ***");
    if let Some(loc) = info.location() {
        println!("at {}:{}", loc.file(), loc.line());
    }
    println!("{}", info.message());
    arch::riscv64::boot::halt()
}
