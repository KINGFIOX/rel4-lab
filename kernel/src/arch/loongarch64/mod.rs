//! LoongArch64 kernel backend.
//!
//! The backend is being brought up incrementally against QEMU `virt`. The boot
//! entry and public arch interfaces compile today; the trap, VSpace, interrupt,
//! and timer implementations are still staging skeletons until the LoongArch
//! seL4 port semantics are wired through.

pub mod boot;
pub mod csr;
pub mod fpu;
pub mod irq;
pub mod paging;
pub mod platform;
pub mod sbi;
pub mod trap;
pub mod vspace;
