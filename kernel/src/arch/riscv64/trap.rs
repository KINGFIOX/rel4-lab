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
///
/// Returns the `UserContext*` of the TCB the kernel wants to resume on
/// the next `sret`. The asm trampoline takes the return value (in a0)
/// straight into `restore_user_context`. By default we re-resume the
/// trapping TCB; the scheduler may override this when a higher-priority
/// TCB has become runnable (or the current one has blocked / been
/// suspended).
#[unsafe(no_mangle)]
pub extern "C" fn handle_trap_rust(uc: &mut UserContext) -> *mut UserContext {
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
                "user fault: scause={:#x} stval={:#x} sepc={:#x} sp={:#x} ra={:#x}",
                cause, stval, uc.pc,
                uc.regs[reg::SP], uc.regs[reg::RA],
            );
            // Until we have a real fault-endpoint IPC, just freeze the
            // offending user thread instead of dragging the kernel down.
            // M3 will turn this into a `seL4_Fault_VMFault` delivery.
            park_current_thread();
        }
    }

    kernel_exit(uc)
}

/// Pick the next TCB to run and return the `UserContext*` to restore.
///
/// Today every priority bin except #255 (the rootserver) is empty in
/// the steady state, so this short-circuits to the trapping TCB. The
/// hook exists so a future `TCB_Resume(higher-prio child)` will land
/// the new thread on `sret` without further plumbing.
#[inline]
fn kernel_exit(uc: &mut UserContext) -> *mut UserContext {
    let cur = crate::object::tcb::current();
    let next = crate::object::tcb::schedule();
    if !next.is_null() && next != cur {
        crate::object::tcb::set_current(next);
        // Future: swap satp + flush TLB here when next has a different
        // VSpace than cur. We don't run multi-VSpace threads yet, so
        // skip — every TCB shares the rootserver's PT.
        return unsafe { &raw mut (*next).context };
    }
    uc as *mut UserContext
}

/// Park the current (only) user thread: spin in S-mode with interrupts
/// disabled. Lets the user inspect the panic message above without QEMU
/// rebooting and without us pretending to handle a fault we can't yet
/// route to a fault endpoint.
fn park_current_thread() -> ! {
    loop {
        unsafe { core::arch::asm!("wfi", options(nomem, nostack)) };
    }
}

/// Walk the rootserver's PT for VA 0x10004000 and assert it still points
/// at PA 0x8034D000 (the boot-time mapping). If a syscall ever stomps on
/// the PT we'll catch it here. Disabled in release; left in until M3.7
/// is debugged.
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
        syscall::SYS_DEBUG_CAP_IDENTIFY => {
            // Returns the cap_tag of the cap at `a0`, with 0 meaning a
            // null cap / unresolvable CPtr. libsel4debug uses this to
            // distinguish "freed slot" from "live cap".
            let cptr = uc.regs[reg::A0];
            let t = unsafe { crate::api::thread::current() };
            let tag = match crate::api::cspace::lookup_cap(t, cptr) {
                Ok((cap, _)) => cap.tag_raw(),
                Err(_) => 0,
            };
            uc.regs[reg::A0] = tag;
        }
        syscall::SYS_DEBUG_DUMP_SCHEDULER
        | syscall::SYS_DEBUG_HALT
        | syscall::SYS_DEBUG_SNAPSHOT
        | syscall::SYS_DEBUG_SEND_IPI => {
            // Debug aids — silently no-op.
        }
        syscall::SYS_YIELD => {
            // Surrender the CPU to any same-priority peer in the
            // runqueue. With only the rootserver in its priority bin
            // this is a no-op (rotate of a singleton); once child TCBs
            // are queued at the same priority it round-robins them.
            unsafe {
                let cur = crate::object::tcb::current();
                if !cur.is_null() {
                    crate::object::tcb::rotate_to_tail(cur);
                }
            }
        }
        syscall::SYS_CALL => {
            crate::api::syscall::do_call(uc);
        }
        syscall::SYS_SEND | syscall::SYS_NB_SEND => {
            crate::api::syscall::do_send(uc, false);
        }
        syscall::SYS_REPLY => {
            // M3.6 will turn Reply into real IPC. For now, no-op.
        }
        syscall::SYS_RECV | syscall::SYS_NB_RECV => {
            let blocking = sysno == syscall::SYS_RECV;
            crate::api::syscall::do_recv(uc, blocking);
        }
        syscall::SYS_REPLY_RECV => {
            // Treat as Recv for now (Reply portion is a no-op until we
            // have a real reply cap path).
            crate::api::syscall::do_recv(uc, true);
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
