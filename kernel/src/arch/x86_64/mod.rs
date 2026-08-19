//! x86_64 kernel backend.
//!
//! Provides the seL4-shaped module contract (`sel4_arch`, `machine`, `object`,
//! `plat`, `smp`) plus user-mode return through `syscall`/`sysret`, a 4-level
//! VSpace, COM1 output, and a single-core x2APIC timer. IPI and IOAPIC stay
//! unwired.

pub mod api;
pub mod kernel;
pub mod machine;
pub mod model;
pub mod object;
pub mod plat;
pub mod sel4_arch;
pub mod smp;
