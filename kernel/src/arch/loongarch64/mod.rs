//! LoongArch64 kernel backend.
//!
//! The backend is being brought up incrementally against QEMU `virt`. It now
//! wires boot entry, user trap/restore, PGDL page-table switching, timer/IPI
//! handling, EXTIOI/PCH interrupt delivery, and kernel/user MMIO attributes
//! into the shared seL4-style kernel paths. Upstream seL4/libsel4/elfloader
//! LoongArch integration is still the external packaging boundary.

pub mod boot;
pub mod csr;
pub mod fpu;
pub mod ipi;
pub mod irq;
pub mod paging;
pub mod platform;
pub mod trap;
pub mod vspace;
