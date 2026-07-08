//! S-mode trap handling: assembly entry, Rust dispatcher, and the
//! `UserContext` shape we save/restore through `sret`.
//!
//! User `ecall`s are decoded as seL4 syscalls. User faults are delivered
//! through the fault IPC path when a fault endpoint is configured; unsupported
//! or kernel-mode traps halt the current thread or panic the kernel.

use core::arch::global_asm;
use core::sync::atomic::{AtomicU64, Ordering};
use log_crate::{error, warn};

use crate::abi::constants::{N_TOTAL_MSG_REGISTERS, WORD_BYTES};
use crate::abi::fault::FaultLabel;
use crate::abi::syscall::SyscallNumber;
use crate::abi::types::MessageInfo;
use crate::api::cspace;
use crate::arch::riscv64::machine::{csr, irq};
use crate::arch::riscv64::object::vspace;
use crate::arch::riscv64::smp::ipi as sbi;
use crate::object::cap::{Cap, CapTag};

/// RISC-V D-extension FPU state shape used by the current `riscv64gc` build.
pub const RISCV_NUM_FP_REGS: usize = 32;
pub const RISCV_FP_REG_BYTES: usize = 8;
pub const RISCV_FPU_STATE_BYTES: usize = (RISCV_NUM_FP_REGS * RISCV_FP_REG_BYTES) + 8;

/// User-mode register snapshot, exactly the layout consumed by `trap.S`.
///
/// Field order is load-bearing: `regs[i]` lives at offset `i * 8`, with
/// `regs[0]` ignored (x0 is hardwired zero), then `pc`, `sstatus`,
/// restart PC, and the RISC-V FPU state.
#[repr(C)]
#[derive(Copy, Clone, Default)]
pub struct FpuState {
    pub regs: [u64; RISCV_NUM_FP_REGS],
    pub fcsr: u32,
    pub _pad: u32,
}

impl FpuState {
    pub const fn zero() -> Self {
        Self {
            regs: [0; RISCV_NUM_FP_REGS],
            fcsr: 0,
            _pad: 0,
        }
    }
}

#[repr(C)]
#[derive(Default)]
pub struct UserContext {
    /// x0..x31. `regs[0]` is unused.
    pub regs: [u64; 32],
    /// Next user PC restored through `sret`.
    pub pc: u64,
    /// Saved sstatus.
    pub sstatus: u64,
    /// seL4 FaultIP/restart PC saved at kernel entry.
    pub restart_pc: u64,
    /// Saved RISC-V FPU registers and fcsr.
    pub fpu: FpuState,
}

const _: () = {
    // 32 GPRs + pc + sstatus + restart_pc + 32 FPRs + fcsr/pad.
    assert!(core::mem::size_of::<UserContext>() == 68 * 8);
    assert!(core::mem::size_of::<FpuState>() == RISCV_FPU_STATE_BYTES);
    assert!(core::mem::offset_of!(UserContext, regs) == 0);
    assert!(core::mem::offset_of!(UserContext, pc) == 32 * 8);
    assert!(core::mem::offset_of!(UserContext, sstatus) == 33 * 8);
    assert!(core::mem::offset_of!(UserContext, restart_pc) == 34 * 8);
    assert!(core::mem::offset_of!(UserContext, fpu) == 35 * 8);
    assert!(core::mem::offset_of!(FpuState, regs) == 0);
    assert!(core::mem::offset_of!(FpuState, fcsr) == RISCV_NUM_FP_REGS * RISCV_FP_REG_BYTES);
};

impl UserContext {
    pub const fn zero() -> Self {
        Self {
            regs: [0; 32],
            pc: 0,
            sstatus: 0,
            restart_pc: 0,
            fpu: FpuState::zero(),
        }
    }
}

/// Register name -> index in `UserContext.regs`.
#[repr(usize)]
#[derive(Copy, Clone, Debug, Eq, PartialEq)]
pub enum UserRegister {
    Ra = 1,
    Sp = 2,
    Gp = 3,
    Tp = 4,
    T0 = 5,
    A0 = 10,
    A1 = 11,
    A2 = 12,
    A3 = 13,
    A4 = 14,
    A5 = 15,
    A6 = 16,
    A7 = 17,
}

impl UserRegister {
    pub const fn index(self) -> usize {
        self as usize
    }
}

/// RISC-V frame registers copied by seL4 TCB register operations.
///
/// Matches upstream `frameRegisters[]`: `FaultIP, ra, sp, gp, s0..s11`.
/// Index 0 is a PC sentinel; the saved value lives in `UserContext.restart_pc`.
pub const SEL4_TCB_FRAME_REGS: [usize; 16] = [
    0,
    UserRegister::Ra.index(),
    UserRegister::Sp.index(),
    UserRegister::Gp.index(),
    8,
    9,
    18,
    19,
    20,
    21,
    22,
    23,
    24,
    25,
    26,
    27,
];

/// RISC-V general-purpose registers copied by seL4 TCB register operations.
///
/// Matches upstream `gpRegisters[]`: `a0..a7, t0..t6, tp`.
pub const SEL4_TCB_GP_REGS: [usize; 16] = [
    UserRegister::A0.index(),
    UserRegister::A1.index(),
    UserRegister::A2.index(),
    UserRegister::A3.index(),
    UserRegister::A4.index(),
    UserRegister::A5.index(),
    UserRegister::A6.index(),
    UserRegister::A7.index(),
    UserRegister::T0.index(),
    6,
    7,
    28,
    29,
    30,
    31,
    UserRegister::Tp.index(),
];

/// Word count of libsel4's RISC-V `seL4_UserContext`.
pub const SEL4_USER_CONTEXT_WORDS: usize = SEL4_TCB_FRAME_REGS.len() + SEL4_TCB_GP_REGS.len();

/// RISC-V `seL4_UserContext` word index to local GPR index.
///
/// Matches libsel4 order:
/// `pc, ra, sp, gp, s0..s11, a0..a7, t0..t6, tp`.
/// Index 0 is a PC sentinel; all other entries are `UserContext.regs[]`
/// indexes.
pub const SEL4_USER_CONTEXT_REGS: [usize; SEL4_USER_CONTEXT_WORDS] = [
    0,
    UserRegister::Ra.index(),
    UserRegister::Sp.index(),
    UserRegister::Gp.index(),
    8,
    9,
    18,
    19,
    20,
    21,
    22,
    23,
    24,
    25,
    26,
    27,
    UserRegister::A0.index(),
    UserRegister::A1.index(),
    UserRegister::A2.index(),
    UserRegister::A3.index(),
    UserRegister::A4.index(),
    UserRegister::A5.index(),
    UserRegister::A6.index(),
    UserRegister::A7.index(),
    UserRegister::T0.index(),
    6,
    7,
    28,
    29,
    30,
    31,
    UserRegister::Tp.index(),
];

pub const SSTATUS_SPIE: u64 = 1 << 5;
pub const SSTATUS_FS_MASK: u64 = 0b11 << 13;
pub const SSTATUS_FS_CLEAN: u64 = 0b10 << 13;
pub const SSTATUS_SUM: u64 = 1 << 18;
pub const USER_SSTATUS: u64 = SSTATUS_SPIE;
pub const ROOTSERVER_SSTATUS: u64 = USER_SSTATUS | SSTATUS_SUM;

global_asm!(include_str!("../trap.S"));

unsafe extern "C" {
    /// Trap vector — must be installed in `stvec`.
    pub fn trap_entry();
    /// Restores the given `UserContext` and `sret`s into user mode.
    /// Never returns.
    pub fn restore_user_context(ctx: *mut UserContext) -> !;
    fn restore_user_context_locked(ctx: *mut UserContext) -> !;
}

pub unsafe fn restore_user_context_with_kernel_lock(
    ctx: *mut UserContext,
    kernel_lock: crate::kernel::smp::KernelLockGuard,
) -> ! {
    program_next_timer();
    kernel_lock.defer_unlock_for_user_restore();
    unsafe { restore_user_context_locked(ctx) }
}

#[repr(usize)]
#[derive(Copy, Clone, Debug, Eq, PartialEq)]
enum ExceptionCode {
    InstructionAccessFault = 1,
    IllegalInstruction = 2,
    LoadAccessFault = 5,
    StoreAccessFault = 7,
    EnvironmentCallFromUser = 8,
    InstructionPageFault = 12,
    LoadPageFault = 13,
    StorePageFault = 15,
}

impl ExceptionCode {
    const fn from_raw(value: usize) -> Option<Self> {
        match value {
            1 => Some(Self::InstructionAccessFault),
            2 => Some(Self::IllegalInstruction),
            5 => Some(Self::LoadAccessFault),
            7 => Some(Self::StoreAccessFault),
            8 => Some(Self::EnvironmentCallFromUser),
            12 => Some(Self::InstructionPageFault),
            13 => Some(Self::LoadPageFault),
            15 => Some(Self::StorePageFault),
            _ => None,
        }
    }

    const fn raw(self) -> usize {
        self as usize
    }
}

#[repr(usize)]
#[derive(Copy, Clone, Debug, Eq, PartialEq)]
enum InterruptCode {
    SupervisorSoftware = 1,
    SupervisorTimer = 5,
    SupervisorExternal = 9,
}

impl InterruptCode {
    const fn from_raw(value: usize) -> Option<Self> {
        match value {
            1 => Some(Self::SupervisorSoftware),
            5 => Some(Self::SupervisorTimer),
            9 => Some(Self::SupervisorExternal),
            _ => None,
        }
    }
}

const SIE_SSIE: usize = 1 << 1;
const SIE_STIE: usize = 1 << 5;
const SIE_SEIE: usize = 1 << 9;
const SIP_SSIP: usize = 1 << 1;
const SCOUNTEREN_TM: usize = 1 << 1;
const TIMER_INTERVAL_TICKS: u64 = 5_000;
const SYNTHETIC_TIMER_IRQ_INTERVAL_TICKS: u64 = 20_000;
const FAULT_MR_REG_COUNT: u64 = 4;

static NEXT_SYNTHETIC_TIMER_IRQ_DEADLINE: AtomicU64 = AtomicU64::new(0);

pub fn init_timer() {
    csr::set_scounteren(csr::scounteren() | SCOUNTEREN_TM);
    csr::set_sie(csr::sie() | SIE_SSIE | SIE_STIE | SIE_SEIE);
    let now = csr::time() as u64;
    if crate::kernel::smp::current_core_id() == 0 {
        NEXT_SYNTHETIC_TIMER_IRQ_DEADLINE.store(
            now.wrapping_add(SYNTHETIC_TIMER_IRQ_INTERVAL_TICKS),
            Ordering::Release,
        );
    }
    program_next_timer();
}

fn synthetic_timer_irq_deadline(now: u64) -> Option<u64> {
    if crate::kernel::smp::current_core_id() != 0 {
        return None;
    }
    let previous = NEXT_SYNTHETIC_TIMER_IRQ_DEADLINE.load(Ordering::Acquire);
    if previous != 0 {
        return Some(previous);
    }
    let deadline = now.wrapping_add(SYNTHETIC_TIMER_IRQ_INTERVAL_TICKS);
    NEXT_SYNTHETIC_TIMER_IRQ_DEADLINE.store(deadline, Ordering::Release);
    Some(deadline)
}

fn program_next_timer() {
    let now = csr::time() as u64;
    let deadline = synthetic_timer_irq_deadline(now);
    let deadline = deadline.unwrap_or_else(|| now.wrapping_add(TIMER_INTERVAL_TICKS));
    crate::kernel::smp::set_next_timer_deadline(deadline);
    sbi::set_timer(deadline);
}

fn should_signal_synthetic_timer_irq(now: u64) -> bool {
    if crate::kernel::smp::current_core_id() != 0 {
        return false;
    }
    let previous = NEXT_SYNTHETIC_TIMER_IRQ_DEADLINE.load(Ordering::Acquire);
    if previous == 0 {
        NEXT_SYNTHETIC_TIMER_IRQ_DEADLINE.store(
            now.wrapping_add(SYNTHETIC_TIMER_IRQ_INTERVAL_TICKS),
            Ordering::Release,
        );
        return false;
    }
    if now < previous {
        return false;
    }
    let mut next = previous;
    while next <= now {
        next = next.wrapping_add(SYNTHETIC_TIMER_IRQ_INTERVAL_TICKS);
    }
    NEXT_SYNTHETIC_TIMER_IRQ_DEADLINE.store(next, Ordering::Release);
    true
}

/// Poll the architectural timer while the kernel is spinning or running a
/// long in-kernel continuation. Hardware timer interrupts are masked while
/// we are in S-mode, so those paths need this explicit delivery point.
pub fn service_due_timer_interrupts() -> bool {
    let deadline = crate::kernel::smp::next_timer_deadline();
    if deadline == 0 {
        return false;
    }
    let now = csr::time() as u64;
    if now < deadline {
        return false;
    }
    handle_timer_interrupt();
    true
}

/// Rust trap dispatcher, called from `trap_entry` once user registers are
/// saved into the supplied `UserContext`.
///
/// Returns the `UserContext*` of the TCB the kernel wants to resume on
/// the next `sret`. The asm trampoline takes the return value (in a0)
/// straight into `restore_user_context`. By default we re-resume the
/// trapping TCB; the scheduler may override this when the next round-robin TCB
/// is runnable or the current one has blocked / been suspended.
#[unsafe(no_mangle)]
pub extern "C" fn handle_trap_rust(uc: *mut UserContext) -> *mut UserContext {
    let kernel_lock = crate::kernel::smp::KernelLockGuard::lock();
    if kernel_lock.remote_stalled_current() {
        handle_software_interrupt();
        return kernel_exit_after_remote_stall(kernel_lock);
    }
    if uc.is_null() {
        panic!("trap entry passed a null user context");
    }
    let uc = unsafe { &mut *uc };
    let cause = csr::scause();
    let stval = csr::stval();
    uc.restart_pc = uc.pc;

    // The high bit of scause distinguishes interrupts (1) from exceptions (0).
    let is_interrupt = (cause as isize) < 0;
    let code = cause & !(1usize << 63);

    if is_interrupt {
        match InterruptCode::from_raw(code) {
            Some(InterruptCode::SupervisorSoftware) => {
                handle_software_interrupt();
                return kernel_exit(uc, kernel_lock);
            }
            Some(InterruptCode::SupervisorTimer) => {
                handle_timer_interrupt();
                return kernel_exit(uc, kernel_lock);
            }
            Some(InterruptCode::SupervisorExternal) => {
                service_pending_external_interrupt();
                return kernel_exit(uc, kernel_lock);
            }
            None => {
                panic!(
                    "unexpected interrupt: scause={:#x} stval={:#x}",
                    cause, stval
                );
            }
        }
    }

    match ExceptionCode::from_raw(code) {
        Some(ExceptionCode::EnvironmentCallFromUser) => handle_syscall(uc),
        _ => {
            if !send_fault_ipc(uc, code, stval as u64) {
                warn!(
                    "user fault: scause={:#x} stval={:#x} sepc={:#x} sp={:#x} ra={:#x}",
                    cause,
                    stval,
                    uc.pc,
                    uc.regs[UserRegister::Sp.index()],
                    uc.regs[UserRegister::Ra.index()],
                );
                park_current_thread();
            }
        }
    }

    kernel_exit(uc, kernel_lock)
}

fn fault_message(code: usize, stval: u64, uc: &UserContext) -> (u64, u64, [u64; 16]) {
    let mut mrs = [0; 16];
    match ExceptionCode::from_raw(code) {
        Some(
            fault @ (ExceptionCode::InstructionAccessFault
            | ExceptionCode::LoadAccessFault
            | ExceptionCode::StoreAccessFault
            | ExceptionCode::InstructionPageFault
            | ExceptionCode::LoadPageFault
            | ExceptionCode::StorePageFault),
        ) => {
            let instruction_fault = matches!(
                fault,
                ExceptionCode::InstructionAccessFault | ExceptionCode::InstructionPageFault
            ) as u64;
            let fsr = match fault {
                ExceptionCode::InstructionAccessFault | ExceptionCode::InstructionPageFault => {
                    ExceptionCode::InstructionAccessFault.raw()
                }
                ExceptionCode::LoadAccessFault | ExceptionCode::LoadPageFault => {
                    ExceptionCode::LoadAccessFault.raw()
                }
                ExceptionCode::StoreAccessFault | ExceptionCode::StorePageFault => {
                    ExceptionCode::StoreAccessFault.raw()
                }
                _ => fault.raw(),
            };
            mrs[0] = uc.pc;
            mrs[1] = stval;
            mrs[2] = instruction_fault;
            mrs[3] = fsr as u64;
            (FaultLabel::VmFault.raw(), 4, mrs)
        }
        _ => {
            mrs[0] = uc.pc;
            mrs[1] = uc.regs[UserRegister::Sp.index()];
            mrs[2] = code as u64;
            mrs[3] = 0;
            (FaultLabel::UserException.raw(), 4, mrs)
        }
    }
}

unsafe fn write_fault_ipc_message(
    receiver: *mut crate::object::tcb::Tcb,
    badge: u64,
    label: u64,
    len: u64,
    mrs: &[u64; 16],
) {
    if receiver.is_null() {
        return;
    }
    let info_word = MessageInfo::new(label, 0, 0, len).0;
    unsafe {
        crate::object::tcb::write_fault_ipc_message_regs(receiver, badge, info_word, mrs, len);
    }

    let copied_len = len.min(mrs.len() as u64);
    if copied_len > FAULT_MR_REG_COUNT {
        unsafe {
            crate::object::tcb::write_ipc_buffer_words(
                receiver,
                1 + FAULT_MR_REG_COUNT as usize,
                &mrs[FAULT_MR_REG_COUNT as usize..copied_len as usize],
            );
        }
    }
}

unsafe fn finish_fault_ipc_receive(
    receiver: *mut crate::object::tcb::Tcb,
    fault_tcb: *mut crate::object::tcb::Tcb,
    handler_cap: crate::object::cap::Cap,
    _reply_rights: bool,
) {
    use crate::object::tcb;

    if receiver.is_null() || fault_tcb.is_null() {
        return;
    }
    unsafe {
        let (reply_cptr, reply_kva, reply_can_grant) = tcb::start_receiver_rendezvous(receiver);
        if crate::api::ipc::set_reply_object_for(
            receiver,
            reply_cptr,
            reply_kva,
            reply_can_grant,
            fault_tcb,
            handler_cap.endpoint_can_grant(),
            false,
        ) {
            tcb::set_blocked_on_reply(fault_tcb, reply_kva);
        } else {
            tcb::set_inactive(fault_tcb);
            tcb::clear_waiting_on(fault_tcb);
        }
        tcb::finish_receiver_rendezvous(receiver);
        tcb::enqueue(receiver);
    }
}

fn send_fault_ipc(uc: &mut UserContext, code: usize, stval: u64) -> bool {
    use crate::object::endpoint;
    use crate::object::tcb;

    let cur = tcb::current();
    if cur.is_null() {
        return false;
    }

    let handler_cap = fault_handler_cap(cur);
    if handler_cap.tag() != Some(CapTag::Endpoint)
        || !handler_cap.endpoint_can_send()
        || !(handler_cap.endpoint_can_grant() || handler_cap.endpoint_can_grant_reply())
    {
        return false;
    }

    let ep = handler_cap.endpoint_ptr() as *mut endpoint::Endpoint;
    if ep.is_null() {
        return false;
    }

    let (label, len, mrs) = fault_message(code, stval, uc);
    unsafe {
        tcb::record_fault_message(cur, label, len, mrs);
        let receiver = {
            let _guard = endpoint::lock_queue(ep);
            let receiver = endpoint::pop_receiver_locked(ep);
            if receiver.is_null() {
                block_fault_sender_locked(
                    cur,
                    ep,
                    handler_cap.endpoint_badge(),
                    handler_cap.endpoint_can_grant(),
                    handler_cap.endpoint_can_grant_reply(),
                    label,
                    len,
                    mrs,
                );
            }
            receiver
        };
        if receiver.is_null() {
            return true;
        }
        write_fault_ipc_message(receiver, handler_cap.endpoint_badge(), label, len, &mrs);
        finish_fault_ipc_receive(receiver, cur, handler_cap, true);
    }
    true
}

pub fn send_cap_fault_ipc(uc: &mut UserContext, addr: u64, in_recv_phase: bool) -> bool {
    let mut mrs = [0; 16];
    mrs[0] = uc.restart_pc;
    mrs[1] = addr;
    mrs[2] = in_recv_phase as u64;
    mrs[3] = 1; // MissingCapability-style lookup failure.
    mrs[4] = 0; // BitsLeft.
    send_synthetic_fault_ipc(FaultLabel::CapFault.raw(), 5, mrs)
}

fn send_unknown_syscall_fault(uc: &mut UserContext, sysno: isize) -> bool {
    let mut mrs = [0; 16];
    mrs[0] = uc.restart_pc;
    mrs[1] = uc.regs[UserRegister::Sp.index()];
    mrs[2] = uc.regs[UserRegister::Ra.index()];
    mrs[3] = uc.regs[UserRegister::A0.index()];
    mrs[4] = uc.regs[UserRegister::A1.index()];
    mrs[5] = uc.regs[UserRegister::A2.index()];
    mrs[6] = uc.regs[UserRegister::A3.index()];
    mrs[7] = uc.regs[UserRegister::A4.index()];
    mrs[8] = uc.regs[UserRegister::A5.index()];
    mrs[9] = uc.regs[UserRegister::A6.index()];
    mrs[10] = sysno as u64;
    send_synthetic_fault_ipc(FaultLabel::UnknownSyscall.raw(), 11, mrs)
}

fn send_synthetic_fault_ipc(label: u64, len: u64, mrs: [u64; 16]) -> bool {
    use crate::object::endpoint;
    use crate::object::tcb;

    let cur = tcb::current();
    if cur.is_null() {
        return false;
    }
    let handler_cap = fault_handler_cap(cur);
    if handler_cap.tag() != Some(CapTag::Endpoint)
        || !handler_cap.endpoint_can_send()
        || !(handler_cap.endpoint_can_grant() || handler_cap.endpoint_can_grant_reply())
    {
        return false;
    }
    let ep = handler_cap.endpoint_ptr() as *mut endpoint::Endpoint;
    if ep.is_null() {
        return false;
    }

    unsafe {
        tcb::record_fault_message(cur, label, len, mrs);
        let receiver = {
            let _guard = endpoint::lock_queue(ep);
            let receiver = endpoint::pop_receiver_locked(ep);
            if receiver.is_null() {
                block_fault_sender_locked(
                    cur,
                    ep,
                    handler_cap.endpoint_badge(),
                    handler_cap.endpoint_can_grant(),
                    handler_cap.endpoint_can_grant_reply(),
                    label,
                    len,
                    mrs,
                );
            }
            receiver
        };
        if receiver.is_null() {
            return true;
        }
        write_fault_ipc_message(receiver, handler_cap.endpoint_badge(), label, len, &mrs);
        finish_fault_ipc_receive(receiver, cur, handler_cap, true);
    }
    true
}

fn fault_handler_cap(tcb: *const crate::object::tcb::Tcb) -> Cap {
    let cptr = crate::object::tcb::fault_endpoint_cptr_snapshot(tcb);
    if cptr != 0 {
        let root = crate::object::tcb::cspace_cap_snapshot(tcb);
        if let Ok((cap, _)) = cspace::lookup_cap_in(root, cptr, cspace::WORD_BITS) {
            return cap;
        }
    }
    crate::object::tcb::fault_endpoint_snapshot(tcb)
}
pub(crate) fn send_timeout_fault_ipc_for(fault_tcb: *mut crate::object::tcb::Tcb) -> bool {
    let _ = fault_tcb;
    false
}

unsafe fn block_fault_sender_locked(
    cur: *mut crate::object::tcb::Tcb,
    ep: *mut crate::object::endpoint::Endpoint,
    badge: u64,
    can_grant: bool,
    can_grant_reply: bool,
    label: u64,
    len: u64,
    mrs: [u64; 16],
) {
    use crate::object::endpoint::{self, EpState};
    use crate::object::tcb;

    if cur.is_null() || ep.is_null() {
        return;
    }
    unsafe {
        tcb::dequeue(cur);
        tcb::set_blocked_fault_sender(
            cur,
            ep as u64,
            badge,
            can_grant,
            can_grant_reply,
            label,
            len,
            mrs,
        );
        endpoint::enqueue_waiter_locked(ep, cur, EpState::Sending);
    }
}

fn handle_timer_interrupt() {
    let now = csr::time() as u64;
    unsafe {
        if should_signal_synthetic_timer_irq(now) {
            crate::object::irq::signal_irq(irq::KERNEL_TIMER_IRQ as u64);
        }
    }
    program_next_timer();
}

fn handle_software_interrupt() {
    csr::set_sip(csr::sip() & !SIP_SSIP);
}

pub fn idle_scheduler_loop() -> ! {
    loop {
        let next_context = {
            let kernel_lock = crate::kernel::smp::KernelLockGuard::lock();
            handle_software_interrupt();
            let _ = service_due_timer_interrupts();
            let next = crate::object::tcb::schedule();
            if next.is_null() {
                crate::kernel::smp::clear_current_state();
                switch_to_kernel_vspace();
                program_next_timer();
                None
            } else {
                crate::object::tcb::set_current(next);
                let ctx = unsafe { crate::object::tcb::prepare_for_user_restore(next) };
                unsafe { switch_to_tcb_vspace(next) };
                program_next_timer();
                Some((ctx, kernel_lock))
            }
        };
        if let Some((ctx, kernel_lock)) = next_context {
            kernel_lock.defer_unlock_for_user_restore();
            unsafe { restore_user_context_locked(ctx) };
        }
        unsafe {
            core::arch::asm!("wfi", options(nomem, nostack));
        }
    }
}

fn switch_to_kernel_vspace() {
    let Some(kernel_satp) = crate::kernel::smp::kernel_satp() else {
        return;
    };
    if csr::satp() as u64 != kernel_satp {
        unsafe { vspace::switch_satp(kernel_satp) };
    }
}

fn service_pending_external_interrupt() -> bool {
    let Some(irq) = irq::claim_external_irq() else {
        return false;
    };
    let delivered = unsafe { crate::object::irq::signal_irq(irq) };
    if !delivered {
        irq::complete_external_irq(irq);
    }
    true
}

/// Program `satp` for the TCB we're about to resume.
///
/// Reads the TCB's VTable CTE slot (a mapped `PageTable` cap whose
/// `base_ptr` is the root PT's kernel VA) and translates that into an Sv39
/// satp value via `vspace::satp_from_kva`. The ASID must already have been
/// assigned through the ASID pool path, matching seL4's VSpace model.
///
/// No-ops when the cap is missing/invalid or when the new satp matches
/// the current one — both common for the rootserver path.
unsafe fn switch_to_tcb_vspace(tcb: *const crate::object::tcb::Tcb) {
    use crate::object::cap::CapTag;
    let vroot = crate::object::tcb::vspace_cap_snapshot(tcb);
    if vroot.tag() != Some(CapTag::PageTable) {
        return;
    }
    let root_kva = vroot.page_table_base_ptr();
    if root_kva == 0 {
        return;
    }
    let asid = vroot.page_table_mapped_asid();
    if !vroot.page_table_is_mapped() || asid == 0 {
        return;
    }
    if crate::object::asid::lookup(asid) != root_kva {
        return;
    }
    let asid = asid as u64;
    let new_satp = vspace::satp_from_kva(root_kva, asid);
    if new_satp == 0 {
        return;
    }
    let cur_satp = csr::satp() as u64;
    if cur_satp != new_satp {
        unsafe { vspace::switch_satp(new_satp) };
    }
}

/// Pick the next TCB to run and return the `UserContext*` to restore.
///
/// Three paths:
/// 1. Round-robin head differs from the trapping TCB → swap.
/// 2. Round-robin head *is* the trapping TCB, or current is runnable and no
///    peer exists → fall through to current.
/// 3. Scheduler returns null AND the trapping TCB is no longer
///    runnable (state != Running) — every thread is blocked. We
///    cannot sret back into the blocked TCB (its caller saw the
///    syscall complete and would resume past it as if it returned
///    a no-op reply). Spin in S-mode WFI until something becomes
///    runnable. With no interrupts wired yet this is functionally
///    a deadlock guard: the test runner's `TIMEOUT` will catch a
///    real deadlock instead of silently corrupting a blocked TCB's
///    user-mode state.
#[inline]
fn kernel_exit(
    uc: &mut UserContext,
    kernel_lock: crate::kernel::smp::KernelLockGuard,
) -> *mut UserContext {
    use crate::object::tcb;
    let cur = tcb::current();

    loop {
        unsafe {
            tcb::enqueue_if_migrated_from_current_core(cur);
            if tcb::take_continue_current_once(cur) && tcb::is_runnable_on_current_core(cur) {
                tcb::prepare_for_user_restore(cur);
                return finish_kernel_exit(uc as *mut UserContext, kernel_lock);
            }
            if tcb::is_runnable_on_current_core(cur) {
                tcb::enqueue(cur);
            }
        }

        let next = tcb::schedule();
        if !next.is_null() {
            if next != cur {
                tcb::set_current(next);
                let ctx = unsafe { tcb::prepare_for_user_restore(next) };
                // Swap satp if `next` lives in a different VSpace.
                // Test processes (sel4test BASIC tests) each spawn into
                // their own root PT; without this swap they'd execute
                // in the driver's VSpace and re-run the driver's
                // libc constructors (re-running `init_syscall_table`
                // hits its `boot_set_tid_address` assertion).
                unsafe { switch_to_tcb_vspace(next) };
                return finish_kernel_exit(ctx, kernel_lock);
            }
            if unsafe { tcb::is_runnable_on_current_core(cur) } {
                unsafe { tcb::prepare_for_user_restore(cur) };
                return finish_kernel_exit(uc as *mut UserContext, kernel_lock);
            }
            continue;
        }

        // schedule() returned null. Safe to fall through *only* if
        // current is still runnable — otherwise we'd resume a blocked
        // TCB's user mode and break IPC semantics.
        let cur_runnable = if !cur.is_null() {
            unsafe { tcb::is_runnable_on_current_core(cur) }
        } else {
            false
        };
        if cur_runnable {
            unsafe { tcb::prepare_for_user_restore(cur) };
            return finish_kernel_exit(uc as *mut UserContext, kernel_lock);
        }

        crate::kernel::smp::clear_current_state();
        switch_to_kernel_vspace();
        drop(kernel_lock);
        idle_scheduler_loop();
    }
}

fn kernel_exit_after_remote_stall(
    kernel_lock: crate::kernel::smp::KernelLockGuard,
) -> *mut UserContext {
    use crate::object::tcb;

    loop {
        let next = tcb::schedule();
        if !next.is_null() {
            tcb::set_current(next);
            let ctx = unsafe { tcb::prepare_for_user_restore(next) };
            unsafe { switch_to_tcb_vspace(next) };
            return finish_kernel_exit(ctx, kernel_lock);
        }

        crate::kernel::smp::clear_current_state();
        switch_to_kernel_vspace();
        drop(kernel_lock);
        idle_scheduler_loop();
    }
}

#[inline]
fn finish_kernel_exit(
    ctx: *mut UserContext,
    kernel_lock: crate::kernel::smp::KernelLockGuard,
) -> *mut UserContext {
    program_next_timer();
    kernel_lock.defer_unlock_for_user_restore();
    ctx
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

fn debug_halt(message: &str) -> ! {
    error!("{message}");
    crate::arch::riscv64::kernel::boot::halt()
}

fn current_ipc_buffer_kva_for_debug() -> u64 {
    let cur = crate::object::tcb::current();
    if !cur.is_null() {
        return crate::object::tcb::ipc_buffer_kva_snapshot(cur);
    }
    unsafe { crate::api::thread::with_current(|thread| thread.ipc_buffer_kva as u64) }
}

fn handle_debug_name_thread(uc: &UserContext) {
    let cptr = uc.regs[UserRegister::A0.index()];
    let cap = match unsafe {
        crate::api::thread::with_current(|thread| crate::api::cspace::lookup_cap(thread, cptr))
    } {
        Ok((cap, _)) => cap,
        Err(_) => debug_halt("SysDebugNameThread: cap is not a TCB, halting"),
    };
    if cap.tag() != Some(CapTag::Thread) {
        debug_halt("SysDebugNameThread: cap is not a TCB, halting");
    }
    let target = crate::object::tcb::from_cap(cap);
    if target.is_null() {
        debug_halt("SysDebugNameThread: cap is not a TCB, halting");
    }

    let ipc_buffer = current_ipc_buffer_kva_for_debug();
    if ipc_buffer == 0 {
        debug_halt("SysDebugNameThread: Failed to lookup IPC buffer, halting");
    }
    let name = unsafe { (ipc_buffer as *const u8).add(WORD_BYTES) };
    let max_len = N_TOTAL_MSG_REGISTERS * WORD_BYTES;
    let mut len = 0;
    while len < max_len {
        if unsafe { *name.add(len) } == 0 {
            unsafe { crate::object::tcb::set_debug_name(target, name, len) };
            return;
        }
        len += 1;
    }
    debug_halt("SysDebugNameThread: Name too long, halting");
}

/// Called when scause = environment call from U-mode.
///
/// On RV64 seL4, the syscall number is passed in `a7` as a signed `isize`.
fn handle_syscall(uc: &mut UserContext) {
    let raw_sysno = uc.regs[UserRegister::A7.index()] as isize;

    // Advance PC past the `ecall` (4 bytes; RVC ecall is 16-bit but the
    // compressed encoding doesn't exist for ecall — it's always 32-bit).
    uc.pc = uc.pc.wrapping_add(4);

    match SyscallNumber::from_raw(raw_sysno) {
        Some(SyscallNumber::DebugPutChar) => {
            let ch = uc.regs[UserRegister::A0.index()] as u8;
            crate::machine::console::putc(ch);
        }
        Some(SyscallNumber::DebugNameThread) => {
            handle_debug_name_thread(uc);
        }
        Some(SyscallNumber::DebugCapIdentify) => {
            // Returns the cap_tag of the cap at `a0`, with 0 meaning a
            // null cap / unresolvable CPtr. libsel4debug uses this to
            // distinguish "freed slot" from "live cap".
            let cptr = uc.regs[UserRegister::A0.index()];
            let tag = match unsafe {
                crate::api::thread::with_current(|thread| {
                    crate::api::cspace::lookup_cap(thread, cptr)
                })
            } {
                Ok((cap, _)) => cap.tag_raw(),
                Err(_) => 0,
            };
            uc.regs[UserRegister::A0.index()] = tag;
        }
        Some(SyscallNumber::DebugHalt) => {
            debug_halt("Debug halt syscall from user thread");
        }
        Some(SyscallNumber::DebugSendIpi) => {
            debug_halt("SysDebugSendIPI: not supported on this architecture");
        }
        Some(SyscallNumber::DebugDumpScheduler | SyscallNumber::DebugSnapshot) => {
            // Debug dumps are diagnostic only. The local kernel does not yet
            // have seL4's scheduler/capDL dump machinery, so these remain
            // success no-ops rather than widening any mutation semantics.
        }
        Some(SyscallNumber::Yield) => unsafe {
            let cur = crate::object::tcb::current();
            if !cur.is_null() {
                crate::object::tcb::rotate_to_tail(cur);
            }
        },
        Some(SyscallNumber::Call) => {
            crate::api::syscall::do_call(uc);
        }
        Some(SyscallNumber::Send) => {
            crate::api::syscall::do_send(uc, false);
        }
        Some(SyscallNumber::NonBlockingSend) => {
            crate::api::syscall::do_send(uc, true);
        }
        Some(SyscallNumber::Reply) => {
            crate::api::ipc::reply(uc);
        }
        Some(SyscallNumber::Recv | SyscallNumber::NonBlockingRecv) => {
            let blocking = SyscallNumber::from_raw(raw_sysno) == Some(SyscallNumber::Recv);
            crate::api::syscall::do_recv(uc, blocking);
        }
        Some(SyscallNumber::ReplyRecv) => {
            crate::api::ipc::reply_recv(uc);
        }
        None => {
            if !send_unknown_syscall_fault(uc, raw_sysno) {
                warn!(
                    "unknown syscall number {} (regs: a0={:#x} a1={:#x} a7={:#x})",
                    raw_sysno,
                    uc.regs[UserRegister::A0.index()],
                    uc.regs[UserRegister::A1.index()],
                    uc.regs[UserRegister::A7.index()]
                );
                park_current_thread();
            }
        }
    }
}

/// Kernel-mode trap panic entry — referenced from `trap.S`.
#[unsafe(no_mangle)]
pub extern "C" fn kernel_trap_panic() -> ! {
    let cause = csr::scause();
    let stval = csr::stval();
    let sepc = csr::sepc();
    error!(
        "kernel-mode trap: scause={:#x} stval={:#x} sepc={:#x}",
        cause, stval, sepc
    );
    panic!("kernel trap");
}

/// Install `trap_entry` as the S-mode trap vector (`stvec`).
pub fn install_trap_vector() {
    let addr = trap_entry as *const () as usize;
    // Direct mode (bits[1:0] = 00).
    debug_assert!(addr & 0x3 == 0, "stvec must be 4-byte aligned");
    csr::set_stvec(addr);
}
