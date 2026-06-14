//! LoongArch64 trap handling, user-context ABI, and syscall/fault IPC path.
//!
//! This module defines the kernel-visible register naming and TCB register
//! ordering, then connects user restore, trap entry, timer/external/IPI
//! interrupts, and user fault delivery.

use core::arch::global_asm;
use core::sync::atomic::{AtomicU64, Ordering};

use log_crate::{error, warn};

use crate::abi::constants::{N_TOTAL_MSG_REGISTERS, WORD_BYTES};
use crate::abi::fault::FaultLabel;
use crate::abi::syscall::SyscallNumber;
use crate::abi::types::MessageInfo;
use crate::arch::loongarch64::csr;
use crate::object::cap::CapTag;

/// User-mode register snapshot shape for the LoongArch64 trap entry.
///
/// `regs[0]` is hardwired zero. The remaining indexes are architectural GPR
/// numbers, matching the LoongArch psABI register names.
#[repr(C)]
#[derive(Default)]
pub struct UserContext {
    pub regs: [u64; 32],
    pub pc: u64,
    pub sstatus: u64,
    pub restart_pc: u64,
    pub trap_record: TrapRecord,
}

const _: () = {
    assert!(core::mem::size_of::<UserContext>() == 39 * 8);
    assert!(core::mem::offset_of!(UserContext, regs) == 0);
    assert!(core::mem::offset_of!(UserContext, pc) == 32 * 8);
    assert!(core::mem::offset_of!(UserContext, sstatus) == 33 * 8);
    assert!(core::mem::offset_of!(UserContext, restart_pc) == 34 * 8);
    assert!(core::mem::offset_of!(UserContext, trap_record) == 35 * 8);
};

impl UserContext {
    pub const fn zero() -> Self {
        Self {
            regs: [0; 32],
            pc: 0,
            sstatus: 0,
            restart_pc: 0,
            trap_record: TrapRecord::zero(),
        }
    }
}

#[repr(C)]
#[derive(Copy, Clone, Default)]
pub struct TrapRecord {
    pub era: u64,
    pub prmd: u64,
    pub estat: u64,
    pub badv: u64,
}

impl TrapRecord {
    pub const fn zero() -> Self {
        Self {
            era: 0,
            prmd: 0,
            estat: 0,
            badv: 0,
        }
    }
}

/// Register name -> index in `UserContext.regs`.
#[repr(usize)]
#[derive(Copy, Clone, Debug, Eq, PartialEq)]
pub enum UserRegister {
    Ra = 1,
    Tp = 2,
    Sp = 3,
    A0 = 4,
    A1 = 5,
    A2 = 6,
    A3 = 7,
    A4 = 8,
    A5 = 9,
    A6 = 10,
    A7 = 11,
    T0 = 12,
    T1 = 13,
    T2 = 14,
    T3 = 15,
    T4 = 16,
    T5 = 17,
    T6 = 18,
    T7 = 19,
    T8 = 20,
    R21 = 21,
    Fp = 22,
    S0 = 23,
    S1 = 24,
    S2 = 25,
    S3 = 26,
    S4 = 27,
    S5 = 28,
    S6 = 29,
    S7 = 30,
    S8 = 31,
}

impl UserRegister {
    pub const fn index(self) -> usize {
        self as usize
    }
}

/// LoongArch64 frame registers for seL4-style TCB copy operations.
///
/// Until an upstream seL4 LoongArch port is vendored locally, the project ABI
/// is `pc` followed by GPR r1..r31. The frame/integer split preserves the
/// existing 16 + 16 seL4 TCB register-operation shape.
pub const SEL4_TCB_FRAME_REGS: [usize; 16] = [
    0,
    UserRegister::Ra.index(),
    UserRegister::Tp.index(),
    UserRegister::Sp.index(),
    UserRegister::A0.index(),
    UserRegister::A1.index(),
    UserRegister::A2.index(),
    UserRegister::A3.index(),
    UserRegister::A4.index(),
    UserRegister::A5.index(),
    UserRegister::A6.index(),
    UserRegister::A7.index(),
    UserRegister::T0.index(),
    UserRegister::T1.index(),
    UserRegister::T2.index(),
    UserRegister::T3.index(),
];

/// LoongArch64 integer registers for seL4-style TCB copy operations.
pub const SEL4_TCB_GP_REGS: [usize; 16] = [
    UserRegister::T4.index(),
    UserRegister::T5.index(),
    UserRegister::T6.index(),
    UserRegister::T7.index(),
    UserRegister::T8.index(),
    UserRegister::R21.index(),
    UserRegister::Fp.index(),
    UserRegister::S0.index(),
    UserRegister::S1.index(),
    UserRegister::S2.index(),
    UserRegister::S3.index(),
    UserRegister::S4.index(),
    UserRegister::S5.index(),
    UserRegister::S6.index(),
    UserRegister::S7.index(),
    UserRegister::S8.index(),
];

pub const SEL4_USER_CONTEXT_WORDS: usize = SEL4_TCB_FRAME_REGS.len() + SEL4_TCB_GP_REGS.len();

/// LoongArch64 `seL4_UserContext` word index to local GPR index.
///
/// Index 0 is the PC sentinel; indexes 1..31 are GPR r1..r31.
pub const SEL4_USER_CONTEXT_REGS: [usize; SEL4_USER_CONTEXT_WORDS] = [
    0,
    UserRegister::Ra.index(),
    UserRegister::Tp.index(),
    UserRegister::Sp.index(),
    UserRegister::A0.index(),
    UserRegister::A1.index(),
    UserRegister::A2.index(),
    UserRegister::A3.index(),
    UserRegister::A4.index(),
    UserRegister::A5.index(),
    UserRegister::A6.index(),
    UserRegister::A7.index(),
    UserRegister::T0.index(),
    UserRegister::T1.index(),
    UserRegister::T2.index(),
    UserRegister::T3.index(),
    UserRegister::T4.index(),
    UserRegister::T5.index(),
    UserRegister::T6.index(),
    UserRegister::T7.index(),
    UserRegister::T8.index(),
    UserRegister::R21.index(),
    UserRegister::Fp.index(),
    UserRegister::S0.index(),
    UserRegister::S1.index(),
    UserRegister::S2.index(),
    UserRegister::S3.index(),
    UserRegister::S4.index(),
    UserRegister::S5.index(),
    UserRegister::S6.index(),
    UserRegister::S7.index(),
    UserRegister::S8.index(),
];

pub const SSTATUS_FS_MASK: u64 = 0;
pub const SSTATUS_FS_CLEAN: u64 = 0;
pub const PRMD_PPLV_MASK: u64 = 0b11;
pub const PRMD_PPLV_USER: u64 = 0b11;
pub const PRMD_PIE: u64 = 1 << 2;
pub const USER_SSTATUS: u64 = PRMD_PPLV_USER | PRMD_PIE;
pub const ROOTSERVER_SSTATUS: u64 = USER_SSTATUS;
const ESTAT_ECODE_SHIFT: usize = 16;
const ESTAT_ECODE_MASK: usize = 0x3f;
const ESTAT_ESUBCODE_SHIFT: usize = 22;
const ESTAT_ESUBCODE_MASK: usize = 0x1ff;
const ESTAT_IS_EXTIOI0: usize = 1 << 2;
const ESTAT_IS_TIMER: usize = 1 << 11;
const ESTAT_IS_IPI: usize = 1 << 12;
const ECFG_LIE_TIMER: usize = 1 << 11;
const TCFG_ENABLE: usize = 1 << 0;
const TCFG_INITVAL_SHIFT: usize = 2;
const EXCCODE_INTERRUPT: usize = 0;
const EXCCODE_PIL: usize = 1;
const EXCCODE_PIS: usize = 2;
const EXCCODE_PIF: usize = 3;
const EXCCODE_PME: usize = 4;
const EXCCODE_PNR: usize = 5;
const EXCCODE_PNX: usize = 6;
const EXCCODE_PPI: usize = 7;
const EXCCODE_ADE: usize = 8;
const EXCCODE_SYSCALL: usize = 11;
const EXSUBCODE_ADEF: usize = 0;
const FAULT_MR_REG_COUNT: u64 = 4;
const VM_FAULT_FSR_INSTRUCTION: u64 = 1;
const VM_FAULT_FSR_LOAD: u64 = 5;
const VM_FAULT_FSR_STORE: u64 = 7;
const TIMER_INTERVAL_TICKS: u64 = 5_000;
const SYNTHETIC_TIMER_IRQ_INTERVAL_TICKS: u64 = 20_000;

static NEXT_SYNTHETIC_TIMER_IRQ_DEADLINE: AtomicU64 = AtomicU64::new(0);

global_asm!(include_str!("trap.S"));

unsafe extern "C" {
    pub fn trap_entry();
    pub fn restore_user_context(ctx: *mut UserContext) -> !;
    fn restore_user_context_locked(ctx: *mut UserContext) -> !;
}

#[unsafe(no_mangle)]
pub extern "C" fn handle_trap_rust(uc: *mut UserContext) -> *mut UserContext {
    let kernel_lock = crate::kernel::smp::KernelLockGuard::lock();
    if kernel_lock.remote_stalled_current() {
        return kernel_exit_after_remote_stall(kernel_lock);
    }
    if uc.is_null() {
        panic!("trap entry passed a null user context");
    }
    let uc = unsafe { &mut *uc };
    let record = uc.trap_record;
    uc.restart_pc = uc.pc;

    let ecode = estat_ecode(record.estat as usize);
    match ecode {
        EXCCODE_SYSCALL => handle_syscall(uc),
        EXCCODE_INTERRUPT => {
            let estat = record.estat as usize;
            let mut serviced_remote_op = false;
            if ipi_pending(estat) {
                match crate::kernel::smp::service_pending_remote_core_op() {
                    crate::kernel::smp::RemoteCoreOpResult::StalledCurrent => {
                        return kernel_exit_after_remote_stall(kernel_lock);
                    }
                    crate::kernel::smp::RemoteCoreOpResult::Serviced => {
                        serviced_remote_op = true;
                    }
                    crate::kernel::smp::RemoteCoreOpResult::None => {}
                }
            }
            if !service_pending_interrupt(estat) && !serviced_remote_op {
                warn!(
                    "loongarch64 unhandled interrupt: estat={:#x} era={:#x}",
                    record.estat, record.era
                );
            }
        }
        _ => {
            let esubcode = estat_esubcode(record.estat as usize);
            if !send_fault_ipc(uc, ecode, esubcode, record.badv) {
                warn!(
                    "loongarch64 user fault: ecode={} estat={:#x} badv={:#x} era={:#x} sp={:#x} ra={:#x}",
                    ecode,
                    record.estat,
                    record.badv,
                    record.era,
                    uc.regs[UserRegister::Sp.index()],
                    uc.regs[UserRegister::Ra.index()],
                );
                park_current_thread();
            }
        }
    }

    kernel_exit(uc, kernel_lock)
}

#[inline]
fn estat_ecode(estat: usize) -> usize {
    (estat >> ESTAT_ECODE_SHIFT) & ESTAT_ECODE_MASK
}

#[inline]
fn estat_esubcode(estat: usize) -> usize {
    (estat >> ESTAT_ESUBCODE_SHIFT) & ESTAT_ESUBCODE_MASK
}

#[inline]
fn timer_pending(estat: usize) -> bool {
    estat & ESTAT_IS_TIMER != 0
}

#[inline]
fn external_irq_pending(estat: usize) -> bool {
    estat & ESTAT_IS_EXTIOI0 != 0
}

fn ipi_pending(estat: usize) -> bool {
    estat & ESTAT_IS_IPI != 0
}

fn fault_message(
    code: usize,
    subcode: usize,
    badv: u64,
    uc: &UserContext,
) -> (u64, u64, [u64; 16]) {
    let mut mrs = [0; 16];
    match vm_fault_fsr(code, subcode) {
        Some((instruction_fault, fsr)) => {
            mrs[0] = uc.pc;
            mrs[1] = badv;
            mrs[2] = instruction_fault as u64;
            mrs[3] = fsr;
            (FaultLabel::VmFault.raw(), 4, mrs)
        }
        None => {
            mrs[0] = uc.pc;
            mrs[1] = uc.regs[UserRegister::Sp.index()];
            mrs[2] = code as u64;
            mrs[3] = subcode as u64;
            (FaultLabel::UserException.raw(), 4, mrs)
        }
    }
}

fn vm_fault_fsr(code: usize, subcode: usize) -> Option<(bool, u64)> {
    match code {
        EXCCODE_PIF | EXCCODE_PNX => Some((true, VM_FAULT_FSR_INSTRUCTION)),
        EXCCODE_PIS | EXCCODE_PME => Some((false, VM_FAULT_FSR_STORE)),
        EXCCODE_PIL | EXCCODE_PNR => Some((false, VM_FAULT_FSR_LOAD)),
        EXCCODE_PPI => Some((false, VM_FAULT_FSR_LOAD)),
        EXCCODE_ADE if subcode == EXSUBCODE_ADEF => Some((true, VM_FAULT_FSR_INSTRUCTION)),
        EXCCODE_ADE => Some((false, VM_FAULT_FSR_LOAD)),
        _ => None,
    }
}

fn send_fault_ipc(uc: &mut UserContext, code: usize, subcode: usize, badv: u64) -> bool {
    let (label, len, mrs) = fault_message(code, subcode, badv, uc);
    send_synthetic_fault_ipc(label, len, mrs)
}

pub fn send_cap_fault_ipc(uc: &mut UserContext, addr: u64, in_recv_phase: bool) -> bool {
    let mut mrs = [0; 16];
    mrs[0] = uc.restart_pc;
    mrs[1] = addr;
    mrs[2] = in_recv_phase as u64;
    mrs[3] = 1;
    mrs[4] = 0;
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
    let handler_cap = tcb::fault_endpoint_snapshot(cur);
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
            tcb::clear_waiting_on(fault_tcb);
        } else {
            tcb::set_inactive(fault_tcb);
            tcb::clear_waiting_on(fault_tcb);
        }
        tcb::finish_receiver_rendezvous(receiver);
        tcb::enqueue(receiver);
    }
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

fn handle_syscall(uc: &mut UserContext) {
    let raw_sysno = uc.regs[UserRegister::A7.index()] as isize;

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
        Some(SyscallNumber::DebugDumpScheduler | SyscallNumber::DebugSnapshot) => {}
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
        Some(SyscallNumber::Recv | SyscallNumber::NonBlockingRecv) => {
            let blocking = SyscallNumber::from_raw(raw_sysno) == Some(SyscallNumber::Recv);
            crate::api::syscall::do_recv_mcs(uc, blocking, true);
        }
        Some(SyscallNumber::Wait | SyscallNumber::NonBlockingWait) => {
            let blocking = SyscallNumber::from_raw(raw_sysno) == Some(SyscallNumber::Wait);
            crate::api::syscall::do_recv_mcs(uc, blocking, false);
        }
        Some(SyscallNumber::ReplyRecv) => {
            crate::api::syscall::do_reply_recv_mcs(uc);
        }
        Some(SyscallNumber::NonBlockingSendRecv) => {
            crate::api::syscall::do_nbsend_recv_mcs(uc, false);
        }
        Some(SyscallNumber::NonBlockingSendWait) => {
            crate::api::syscall::do_nbsend_recv_mcs(uc, true);
        }
        None => {
            if !send_unknown_syscall_fault(uc, raw_sysno) {
                warn!(
                    "unknown loongarch64 syscall number {} (a0={:#x} a1={:#x} a7={:#x})",
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

fn current_ipc_buffer_kva_for_debug() -> u64 {
    let cur = crate::object::tcb::current();
    if !cur.is_null() {
        return crate::object::tcb::ipc_buffer_kva_snapshot(cur);
    }
    unsafe { crate::api::thread::with_current(|thread| thread.ipc_buffer_kva as u64) }
}

fn debug_halt(message: &str) -> ! {
    error!("{message}");
    crate::arch::loongarch64::boot::halt()
}

fn kernel_exit(
    uc: &mut UserContext,
    kernel_lock: crate::kernel::smp::KernelLockGuard,
) -> *mut UserContext {
    use crate::object::tcb;
    let cur = tcb::current();

    loop {
        unsafe {
            tcb::enqueue_if_migrated_from_current_core(cur);
            if tcb::is_runnable_on_current_core(cur) {
                tcb::enqueue(cur);
            }
        }

        let next = tcb::schedule();
        if !next.is_null() {
            if next != cur {
                tcb::set_current(next);
                let ctx = unsafe { tcb::prepare_for_user_restore(next) };
                switch_to_tcb_vspace(next);
                return finish_kernel_exit(ctx, kernel_lock);
            }
            unsafe { tcb::prepare_for_user_restore(cur) };
            return finish_kernel_exit(uc as *mut UserContext, kernel_lock);
        }

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
            switch_to_tcb_vspace(next);
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

fn switch_to_kernel_vspace() {
    let Some(kernel_satp) = crate::kernel::smp::kernel_satp() else {
        return;
    };
    if crate::arch::loongarch64::vspace::current_satp() != kernel_satp {
        unsafe { crate::arch::loongarch64::vspace::switch_satp(kernel_satp) };
    }
}

/// Program the LoongArch PGDL/ASID pair for the TCB we're about to resume.
///
/// The VSpace cap must already be mapped through an ASID pool, matching the
/// same seL4 root-switching contract as the RISC-V backend.
fn switch_to_tcb_vspace(tcb: *const crate::object::tcb::Tcb) {
    use crate::object::cap::CapTag;

    if tcb.is_null() {
        return;
    }
    let vroot = crate::object::tcb::vspace_cap_snapshot(tcb);
    if vroot.tag() != Some(CapTag::PageTable) {
        return;
    }
    let root_kva = vroot.page_table_base_ptr();
    let asid = vroot.page_table_mapped_asid();
    if root_kva == 0 || !vroot.page_table_is_mapped() || asid == 0 {
        return;
    }
    if crate::object::asid::lookup(asid) != root_kva {
        return;
    }
    let new_satp = crate::arch::loongarch64::vspace::satp_from_kva(root_kva, asid as u64);
    if new_satp == 0 {
        return;
    }
    if crate::arch::loongarch64::vspace::current_satp() != new_satp {
        unsafe { crate::arch::loongarch64::vspace::switch_satp(new_satp) };
    }
}

fn park_current_thread() -> ! {
    loop {
        unsafe { core::arch::asm!("idle 0", options(nomem, nostack)) };
    }
}

#[unsafe(no_mangle)]
pub extern "C" fn kernel_trap_panic() -> ! {
    let estat = csr::estat();
    let badv = csr::badv();
    let era = csr::era();
    error!(
        "loongarch64 kernel trap: estat={:#x} badv={:#x} era={:#x}",
        estat, badv, era
    );
    panic!("kernel trap");
}

pub fn install_trap_vector() {
    let addr = trap_entry as *const () as usize;
    debug_assert!(addr & 0x3f == 0, "eentry must be 64-byte aligned");
    csr::set_eentry(addr);
    csr::ibar();
}

pub fn init_timer() {
    csr::set_ecfg(csr::ecfg() | ECFG_LIE_TIMER);
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

    let delta = deadline.saturating_sub(now).max(1);
    let initval = delta.min((usize::MAX >> TCFG_INITVAL_SHIFT) as u64) as usize;
    csr::set_tcfg((initval << TCFG_INITVAL_SHIFT) | TCFG_ENABLE);
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

fn clear_timer_interrupt() {
    csr::set_ticlr(1);
}

fn handle_timer_interrupt() {
    clear_timer_interrupt();
    let now = csr::time() as u64;
    unsafe {
        if should_signal_synthetic_timer_irq(now) {
            crate::object::irq::signal_irq(super::irq::KERNEL_TIMER_IRQ as u64);
        }
    }
    program_next_timer();
}

fn service_pending_interrupt(estat: usize) -> bool {
    let mut serviced = false;
    if timer_pending(estat) {
        handle_timer_interrupt();
        serviced = true;
    }
    if ipi_pending(estat) {
        serviced |= super::sbi::ack_ipi();
    }
    if external_irq_pending(estat) {
        serviced |= service_pending_external_interrupt();
    }
    serviced
}

fn service_pending_external_interrupt() -> bool {
    let Some(irq) = super::irq::claim_external_irq() else {
        return false;
    };
    let delivered = unsafe { crate::object::irq::signal_irq(irq) };
    if !delivered {
        super::irq::complete_external_irq(irq);
    }
    true
}

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

pub unsafe fn restore_user_context_with_kernel_lock(
    ctx: *mut UserContext,
    kernel_lock: crate::kernel::smp::KernelLockGuard,
) -> ! {
    program_next_timer();
    kernel_lock.defer_unlock_for_user_restore();
    unsafe { restore_user_context_locked(ctx) }
}

pub fn idle_scheduler_loop() -> ! {
    loop {
        let next_context = {
            let kernel_lock = crate::kernel::smp::KernelLockGuard::lock();
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
                switch_to_tcb_vspace(next);
                program_next_timer();
                Some((ctx, kernel_lock))
            }
        };
        if let Some((ctx, kernel_lock)) = next_context {
            kernel_lock.defer_unlock_for_user_restore();
            unsafe { restore_user_context_locked(ctx) };
        }
        unsafe {
            core::arch::asm!("idle 0", options(nomem, nostack));
        }
    }
}
