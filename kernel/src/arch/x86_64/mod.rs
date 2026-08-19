//! x86_64 kernel backend.
//!
//! This backend is intentionally staged behind compile-time support first. It
//! provides the seL4-shaped module contract (`sel4_arch`, `machine`, `object`,
//! `plat`, `smp`) so shared kernel code stays ISA-neutral. Trap, timer, and
//! IPI paths are still stubs.

pub mod api;
pub mod kernel;
pub mod machine;
pub mod model;
pub mod object;
pub mod plat;
pub mod sel4_arch;
pub mod smp;
