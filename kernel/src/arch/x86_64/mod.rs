//! x86_64 kernel backend.
//!
//! This backend is intentionally staged behind compile-time support first. It
//! provides the same module contract as the existing RISC-V and LoongArch
//! backends so shared kernel code can be refactored against an explicit
//! architecture surface before the full x86_64 trap and VSpace path is wired.

pub mod boot;
pub mod csr;
pub mod fpu;
pub mod ipi;
pub mod irq;
pub mod paging;
pub mod platform;
pub mod trap;
pub mod vspace;
