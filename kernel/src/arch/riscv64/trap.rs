//! S-mode trap handling: assembly entry, Rust dispatcher, and the
//! `UserContext` shape we save/restore through `sret`.
//!
//! For M2.2 the only user-mode trap we recognise is an `ecall` carrying a
//! known seL4 syscall number in `a7`. Other exceptions panic the kernel.

use core::arch::global_asm;

use crate::abi::syscall::{self, SyscallNo};
use crate::arch::riscv64::{csr, sbi};

/// User-mode register snapshot, exactly the layout consumed by `trap.S`.
///
/// Field order is load-bearing: `regs[i]` lives at offset `i * 8`, with
/// `regs[0]` ignored (x0 is hardwired zero), then `pc`, `sstatus`, and
/// `sscratch_saved`.
#[repr(C)]
#[derive(Default)]
pub struct UserContext {
    /// x0..x31. `regs[0]` is unused.
    pub regs: [u64; 32],
    /// Saved sepc (user PC at trap).
    pub pc: u64,
    /// Saved sstatus.
    pub sstatus: u64,
    /// Reserved slot — keeps the trampoline asm offsets clean.
    pub _reserved: u64,
}

const _: () = {
    // 32 GPRs + pc + sstatus + reserved = 35 words = 280 bytes
    assert!(core::mem::size_of::<UserContext>() == 35 * 8);
};

/// Register name → index in `UserContext.regs`.
#[allow(dead_code)]
pub mod reg {
    pub const RA: usize = 1;
    pub const SP: usize = 2;
    pub const GP: usize = 3;
    pub const TP: usize = 4;
    pub const T0: usize = 5;
    pub const A0: usize = 10;
    pub const A1: usize = 11;
    pub const A2: usize = 12;
    pub const A3: usize = 13;
    pub const A4: usize = 14;
    pub const A5: usize = 15;
    pub const A6: usize = 16;
    pub const A7: usize = 17;
}

global_asm!(include_str!("trap.S"));

unsafe extern "C" {
    /// Trap vector — must be installed in `stvec`.
    pub fn trap_entry();
    /// Restores the given `UserContext` and `sret`s into user mode.
    /// Never returns.
    pub fn restore_user_context(ctx: *mut UserContext) -> !;
}

/// scause codes we care about.
mod scause_code {
    pub const ENV_CALL_FROM_U: usize = 8;
}

/// Rust trap dispatcher, called from `trap_entry` once user registers are
/// saved into the supplied `UserContext`.
#[unsafe(no_mangle)]
pub extern "C" fn handle_trap_rust(uc: &mut UserContext) {
    let cause = csr::scause();
    let stval = csr::stval();

    // The high bit of scause distinguishes interrupts (1) from exceptions (0).
    let is_interrupt = (cause as isize) < 0;
    let code = cause & !(1usize << 63);

    if is_interrupt {
        // No interrupts handled in M2; just panic for now.
        panic!("unexpected interrupt: scause={:#x} stval={:#x}", cause, stval);
    }

    match code {
        scause_code::ENV_CALL_FROM_U => handle_syscall(uc),
        _ => {
            crate::println!(
                "user trap: scause={:#x} stval={:#x} sepc={:#x}",
                cause,
                stval,
                uc.pc
            );
            panic!("unhandled exception from user mode");
        }
    }
}

/// Called when scause = environment call from U-mode.
///
/// On RV64 seL4, the syscall number is passed in `a7` as a signed `isize`.
fn handle_syscall(uc: &mut UserContext) {
    let sysno = uc.regs[reg::A7] as SyscallNo;

    // Advance PC past the `ecall` (4 bytes; RVC ecall is 16-bit but the
    // compressed encoding doesn't exist for ecall — it's always 32-bit).
    uc.pc = uc.pc.wrapping_add(4);

    match sysno {
        syscall::SYS_DEBUG_PUT_CHAR => {
            let ch = uc.regs[reg::A0] as u8;
            sbi::console_putchar(ch);
        }
        syscall::SYS_DEBUG_NAME_THREAD => {
            // No-op for M2.2. The name is read from the IPC buffer in the
            // real seL4 kernel; we silently accept.
        }
        syscall::SYS_DEBUG_DUMP_SCHEDULER
        | syscall::SYS_DEBUG_HALT
        | syscall::SYS_DEBUG_CAP_IDENTIFY
        | syscall::SYS_DEBUG_SNAPSHOT
        | syscall::SYS_DEBUG_SEND_IPI => {
            // Debug aids — silently no-op.
        }
        syscall::SYS_YIELD => {
            // Single-thread for M2: yield is a no-op.
        }
        n if syscall::is_known(n) => {
            crate::println!(
                "syscall {} not implemented yet (a0={:#x}, a1={:#x})",
                n,
                uc.regs[reg::A0],
                uc.regs[reg::A1]
            );
            panic!("unimplemented syscall {}", n);
        }
        n => {
            crate::println!(
                "unknown syscall number {} (regs: a0={:#x} a1={:#x} a7={:#x})",
                n,
                uc.regs[reg::A0],
                uc.regs[reg::A1],
                uc.regs[reg::A7]
            );
            panic!("unknown syscall {}", n);
        }
    }
}

/// Kernel-mode trap panic stub — referenced from `trap.S`.
#[unsafe(no_mangle)]
pub extern "C" fn kernel_trap_panic() -> ! {
    let cause = csr::scause();
    let stval = csr::stval();
    let sepc = csr::sepc();
    crate::println!(
        "kernel-mode trap: scause={:#x} stval={:#x} sepc={:#x}",
        cause,
        stval,
        sepc
    );
    panic!("kernel trap (M2: not handled)");
}

/// Install `trap_entry` as the S-mode trap vector (`stvec`).
pub fn install_trap_vector() {
    let addr = trap_entry as *const () as usize;
    // Direct mode (bits[1:0] = 00).
    debug_assert!(addr & 0x3 == 0, "stvec must be 4-byte aligned");
    csr::set_stvec(addr);
    // Make sure sscratch starts at 0 so a kernel-mode trap before the
    // first restore_user_context takes the from-kernel panic path.
    csr::set_sscratch(0);
}
