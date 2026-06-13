//! LoongArch64 kernel backend placeholder.
//!
//! The repository tooling can now select `ARCH=loongarch64`, but the kernel
//! backend intentionally remains blocked until the matching seL4 LoongArch64
//! ABI, libsel4 headers, and elfloader platform are available. Implementing
//! this module requires the full boot, trap, timer, IRQ, TLB, page-table, and
//! user-context ABI surface to be derived from that seL4 port.

pub mod platform;

compile_error!(
    "LoongArch64 kernel backend is not implemented yet; add the seL4 LoongArch64 ABI/elfloader port before building this target"
);
