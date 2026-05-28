//! Kernel API layer: turns user-mode `ecall` traps into capability
//! invocations.
//!
//! The flow on `seL4_Call`:
//!
//! ```text
//! user trap → handle_syscall (arch) → api::syscall::do_call
//!   ├─ resolve `a0` through the current thread's CSpace
//!   ├─ dispatch by capability tag → api::invocation::*
//!   ├─ write reply (label/length) into a1, message regs into a2..a5
//!   └─ return; arch layer restores user context and sret
//! ```

pub mod cspace;
pub mod invocation;
pub mod ipc;
pub mod syscall;
pub mod thread;
