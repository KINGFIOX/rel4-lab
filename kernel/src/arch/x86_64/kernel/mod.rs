pub mod boot;
pub mod trap;
pub mod trap_scratch;

pub use trap_scratch::{TrapScratch, TrapScratchCell, init_trap_scratch};

pub const BOOT_PROFILE: &str = "x86_64 long mode";

pub fn start_application_processors() {
    crate::arch::x86_64::smp::start_application_processors();
}
