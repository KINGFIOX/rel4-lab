//! RISC-V 64-bit kernel backend.
//!
//! The public layout mirrors seL4's architecture split: ABI-facing `api`,
//! CPU/kernel entry code in `kernel`, hardware operations in `machine`,
//! kernel objects in `object`, SMP/IPI support in `smp`, and platform data in
//! `plat`.

pub mod api;
pub mod kernel;
pub mod machine;
pub mod model;
pub mod object;
pub mod plat;
pub mod smp;
