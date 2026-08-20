pub mod boot;
pub mod trap;
pub mod trap_scratch;

pub use trap_scratch::{TrapScratchCell, current_trap_scratch, init_trap_scratch};

pub const BOOT_PROFILE: &str = "S-mode, Sv39";

pub fn start_application_processors() {}
