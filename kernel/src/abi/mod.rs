//! Binary-level ABI shared with seL4 userspace (`libsel4`).
//!
//! Every constant and `#[repr(C)]` struct in this module must byte-match the
//! C definitions of the official seL4 kernel for our `qemu-riscv-virt` /
//! MCS build configuration, otherwise the existing `sel4test-driver` binary
//! that we boot in user mode will read garbage.
//!
//! Source of truth (do NOT silently diverge):
//! - `kernel/libsel4/include/sel4/bootinfo_types.h`
//! - `kernel/libsel4/include/sel4/shared_types_gen.h`
//! - `kernel/libsel4/sel4_arch_include/riscv64/sel4/sel4_arch/constants.h`
//! - `build-riscv64/kernel/gen_config/kernel/gen_config.h`
//! - `kernel/libsel4/include/api/syscall.xml`

pub mod bootinfo;
pub mod constants;
pub mod fault;
pub mod syscall;
pub mod types;
