//! x86_64 kernel backend.
//!
//! This backend is intentionally staged behind compile-time support first. It
//! provides the same module contract as the existing RISC-V and LoongArch
//! backends so shared kernel code can be refactored against an explicit
//! architecture surface before the full x86_64 trap and VSpace path is wired.

pub mod api;
pub mod kernel;
pub mod machine;
pub mod model;
pub mod object;
pub mod plat;
pub mod smp;
