//! Slow-path syscall dispatch (Call / Send / Recv / ReplyRecv …).
//!
//! For M3 we only handle `seL4_Call`, since that's what the rootserver
//! uses to drive cap invocations during bootstrap. The arch trap handler
//! decodes the syscall number and routes here; we read message registers
//! from the `UserContext`, perform the invocation, and write the reply
//! back into the same context before returning to user mode.

#![allow(dead_code)]

use crate::abi::constants::N_MSG_REGISTERS;
use crate::abi::types::MessageInfo;
use crate::api::cspace::lookup_cap;
use crate::api::invocation;
use crate::api::thread;
use crate::arch::riscv64::trap::{UserContext, UserRegister};
use crate::object::cap::CapTag;

#[derive(Copy, Clone, Debug)]
pub enum SyscallError {
    InvalidArgument,
    InvalidCapability,
    IllegalOperation,
    RangeError,
    AlignmentError,
    NotEnoughMemory,
    DeleteFirst,
    RevokeFirst,
    TruncatedMessage,
    FailedLookup,
    Unsupported,
    Preempted,
}

/// `seL4_Error` labels from `libsel4/include/sel4/errors.h`.
#[repr(u64)]
#[derive(Copy, Clone, Debug, Eq, PartialEq)]
pub enum SeL4Error {
    InvalidArgument = 1,
    InvalidCapability = 2,
    IllegalOperation = 3,
    RangeError = 4,
    AlignmentError = 5,
    FailedLookup = 6,
    TruncatedMessage = 7,
    DeleteFirst = 8,
    RevokeFirst = 9,
    NotEnoughMemory = 10,
}

impl SeL4Error {
    pub const fn raw(self) -> u64 {
        self as u64
    }
}

impl SyscallError {
    pub fn to_label(self) -> u64 {
        match self {
            Self::InvalidArgument => SeL4Error::InvalidArgument.raw(),
            Self::InvalidCapability => SeL4Error::InvalidCapability.raw(),
            Self::IllegalOperation => SeL4Error::IllegalOperation.raw(),
            Self::RangeError => SeL4Error::RangeError.raw(),
            Self::AlignmentError => SeL4Error::AlignmentError.raw(),
            Self::FailedLookup => SeL4Error::FailedLookup.raw(),
            Self::TruncatedMessage => SeL4Error::TruncatedMessage.raw(),
            Self::DeleteFirst => SeL4Error::DeleteFirst.raw(),
            Self::RevokeFirst => SeL4Error::RevokeFirst.raw(),
            Self::NotEnoughMemory => SeL4Error::NotEnoughMemory.raw(),
            // No seL4_Error code for "not implemented" — use IllegalOperation.
            Self::Unsupported | Self::Preempted => SeL4Error::IllegalOperation.raw(),
        }
    }
}

/// Handle `seL4_Call`: cap lookup + invocation dispatch.
pub fn do_call(uc: &mut UserContext) {
    let cptr = uc.regs[UserRegister::A0.index()];
    let raw_info = uc.regs[UserRegister::A1.index()];
    let info = MessageInfo(raw_info);

    let mut endpoint_call = false;
    let mut success_reply_length = 0;
    let result = unsafe {
        thread::with_current(|t| {
            let lookup_res = lookup_cap(t, cptr);
            let (cap, slot) = match lookup_res {
                Ok(v) => v,
                Err(_) => {
                    return Err(SyscallError::InvalidCapability);
                }
            };

            let tag = cap.tag();
            let label = info.label();
            let mut length = info.length();
            if length > N_MSG_REGISTERS as u64 && !thread::current_has_ipc_buffer() {
                length = N_MSG_REGISTERS as u64;
            }

            let result = match tag {
                Some(CapTag::Untyped) => {
                    invocation::handle_untyped(t, slot, cap, label, length, uc)
                }
                Some(CapTag::CNode) => invocation::handle_cnode(t, slot, cap, label, length, uc),
                Some(CapTag::Frame) => invocation::handle_frame(t, slot, cap, label, length, uc),
                Some(CapTag::PageTable) => {
                    invocation::handle_page_table(t, slot, cap, label, length, uc)
                }
                Some(CapTag::Thread) => invocation::handle_thread(t, slot, cap, label, length, uc),
                Some(CapTag::Endpoint) => {
                    if !cap.endpoint_can_send() {
                        return Err(SyscallError::InvalidCapability);
                    }
                    endpoint_call = true;
                    Ok(())
                }
                Some(CapTag::Null) => Err(SyscallError::InvalidCapability),
                Some(CapTag::Domain) => invocation::handle_domain(t, cap, label, length, uc),
                Some(CapTag::AsidControl) => {
                    invocation::handle_asid_control(t, cap, label, length, uc)
                }
                Some(CapTag::AsidPool) => invocation::handle_asid_pool(t, cap, label, length, uc),
                Some(CapTag::IrqControl) => {
                    invocation::handle_irq_control(t, slot, cap, label, length, uc)
                }
                Some(CapTag::IrqHandler) => {
                    invocation::handle_irq_handler(t, cap, label, length, uc)
                }
                Some(CapTag::SchedControl) => {
                    invocation::handle_sched_control(t, cap, label, length, uc)
                }
                Some(CapTag::SchedContext) => {
                    invocation::handle_sched_context(t, cap, label, length, uc)
                }
                None => Err(SyscallError::InvalidCapability),
                _ => Err(SyscallError::IllegalOperation),
            };
            if result.is_ok() {
                success_reply_length = invocation::success_reply_length(tag, label);
            }
            result
        })
    };
    if endpoint_call {
        // Endpoint Call is a real IPC send. A successful MCS reply will later
        // arrive through an explicit Reply cap, so we do not write the normal
        // invocation reply here.
        crate::api::ipc::call(uc);
        return;
    }

    match result {
        Ok(()) => write_ok_reply(uc, 0, success_reply_length),
        Err(SyscallError::Preempted) => restart_current_invocation_after_preemption(uc),
        Err(e) => write_error_reply(uc, e),
    }
}

fn restart_current_invocation_after_preemption(uc: &mut UserContext) {
    let current = crate::object::tcb::current();
    let (runnable, _) = crate::object::tcb::runnable_sched_context_snapshot(current);
    if runnable {
        uc.pc = uc.restart_pc;
    }
}

fn write_ok_reply(uc: &mut UserContext, label: u64, length: u64) {
    uc.regs[UserRegister::A0.index()] = 0; // badge
    uc.regs[UserRegister::A1.index()] = MessageInfo::new(label, 0, 0, length).0;
    // Don't touch a2..a5: leaving them as the user wrote matches the C
    // kernel's contract for "no extra reply mrs".
}

fn write_error_reply(uc: &mut UserContext, e: SyscallError) {
    // User code routinely invokes Call on caps that don't support the
    // requested label (e.g. SYSCALL0005 `seL4_Call` on the root CNode
    // cap); the error reply *is* the expected behaviour. Don't spam the
    // log — set the label and let the caller read it.
    uc.regs[UserRegister::A0.index()] = 0;
    uc.regs[UserRegister::A1.index()] = MessageInfo::new(e.to_label(), 0, 0, 0).0;
}

/// `seL4_Send` / `seL4_NBSend`: dispatch by cap type.
///
/// For Notification caps this becomes a `sendSignal` call. For
/// Endpoint caps we walk the EP state machine in `api::ipc::send`.
/// Other cap kinds (the test suite Sends to CNodes / Untyped during
/// SYSCALL0001/0002/0004) are silently dropped to match the C kernel.
pub fn do_send(uc: &mut UserContext, nb: bool) {
    let cptr = uc.regs[UserRegister::A0.index()];
    let (cap, slot) = match unsafe { thread::with_current(|t| lookup_cap(t, cptr)) } {
        Ok(v) => v,
        Err(_) => return,
    };

    match cap.tag() {
        Some(CapTag::Notification) => {
            if !cap.notification_can_send() {
                return;
            }
            let ntfn_ptr = cap.notification_ptr() as *mut crate::object::notification::Notification;
            let badge = cap.notification_badge();
            unsafe {
                crate::object::notification::signal(ntfn_ptr, badge);
            }
        }
        Some(CapTag::Endpoint) => {
            crate::api::ipc::send(uc, !nb, false);
        }
        Some(CapTag::Reply) => unsafe {
            if cap.reply_is_object() {
                let reply = cap.reply_object_ptr();
                if reply == 0 {
                    return;
                }
                let caller = crate::object::reply::tcb(reply);
                if crate::object::tcb::blocked_on_reply_snapshot(caller) {
                    crate::object::reply::remove(reply, caller);
                    crate::api::ipc::reply_to_tcb(uc, caller);
                    crate::object::tcb::clear_reply_slot_if(caller, slot as u64);
                }
            }
        },
        _ => {}
    }
}

/// `seL4_Recv` / `seL4_NBRecv`: dispatch by cap type.
///
/// For Notification caps this becomes a `receiveSignal`; for Endpoint caps
/// we walk the EP state machine in `api::ipc::recv`. Invalid receive caps
/// raise a receive-phase CapFault, matching seL4 `handleRecv`.
pub fn do_recv(uc: &mut UserContext, blocking: bool) {
    do_recv_inner(uc, blocking, 0, false)
}

pub fn do_recv_mcs(uc: &mut UserContext, blocking: bool, can_reply: bool) {
    let reply_cptr = if can_reply {
        uc.regs[UserRegister::A6.index()]
    } else {
        0
    };
    do_recv_inner(uc, blocking, reply_cptr, can_reply)
}

fn do_recv_inner(uc: &mut UserContext, blocking: bool, reply_cptr: u64, can_reply: bool) {
    let cptr = uc.regs[UserRegister::A0.index()];
    let cap = match unsafe { thread::with_current(|t| lookup_cap(t, cptr)) } {
        Ok((cap, _slot)) => cap,
        Err(_) => {
            write_recv_cap_fault_or_empty(uc, cptr);
            return;
        }
    };

    match cap.tag() {
        Some(CapTag::Endpoint) => {
            if can_reply {
                if !valid_reply_cap_for_recv(reply_cptr) {
                    write_recv_cap_fault_or_empty(uc, reply_cptr);
                    return;
                }
                crate::api::ipc::recv_mcs(uc, blocking, reply_cptr);
            } else {
                crate::api::ipc::recv(uc, blocking);
            }
        }
        Some(CapTag::Notification) => {
            let ntfn_ptr = cap.notification_ptr() as *mut crate::object::notification::Notification;
            let cur_tcb = crate::object::tcb::current();
            let bound_tcb = unsafe { crate::object::notification::bound_tcb_snapshot(ntfn_ptr) };
            if !cap.notification_can_receive() || (!bound_tcb.is_null() && bound_tcb != cur_tcb) {
                write_recv_cap_fault_or_empty(uc, cptr);
                return;
            }
            let outcome = unsafe { crate::object::notification::wait(ntfn_ptr, cur_tcb, blocking) };
            match outcome {
                crate::object::notification::WaitOutcome::Got(badge) => {
                    uc.regs[UserRegister::A0.index()] = badge;
                    uc.regs[UserRegister::A1.index()] = 0;
                    thread::zero_current_ipc_buffer_words(1, 4);
                    uc.regs[UserRegister::A2.index()] = 0;
                    uc.regs[UserRegister::A3.index()] = 0;
                    uc.regs[UserRegister::A4.index()] = 0;
                    uc.regs[UserRegister::A5.index()] = 0;
                }
                crate::object::notification::WaitOutcome::Blocked => {
                    // Caller is now BlockedOnNotification; signal() will
                    // write its registers when it wakes.
                }
            }
        }
        _ => write_recv_cap_fault_or_empty(uc, cptr),
    }
}

fn valid_reply_cap_for_recv(reply_cptr: u64) -> bool {
    let cap = match unsafe { thread::with_current(|t| lookup_cap(t, reply_cptr)) } {
        Ok((cap, _slot)) => cap,
        Err(_) => return false,
    };
    cap.tag() == Some(CapTag::Reply) && cap.reply_is_object()
}

pub fn do_reply_recv_mcs(uc: &mut UserContext) {
    let reply_cptr = uc.regs[UserRegister::A6.index()];
    if reply_cptr != 0 {
        let saved_cptr = uc.regs[UserRegister::A0.index()];
        uc.regs[UserRegister::A0.index()] = reply_cptr;
        do_send(uc, false);
        uc.regs[UserRegister::A0.index()] = saved_cptr;
    } else {
        crate::api::ipc::reply(uc);
    }
    do_recv_inner(uc, true, reply_cptr, true);
}

pub fn do_nbsend_recv_mcs(uc: &mut UserContext, wait: bool) {
    let src = uc.regs[UserRegister::A0.index()];
    let reply_or_dest = uc.regs[UserRegister::A6.index()];
    let send_dest = if wait { reply_or_dest } else { read_t0(uc) };
    let saved_src = src;
    if send_dest != 0 {
        uc.regs[UserRegister::A0.index()] = send_dest;
        do_send_with_donation(uc, true);
    }
    uc.regs[UserRegister::A0.index()] = saved_src;
    do_recv_inner(uc, true, if wait { 0 } else { reply_or_dest }, !wait);
}

fn do_send_with_donation(uc: &mut UserContext, nb: bool) {
    let cptr = uc.regs[UserRegister::A0.index()];
    let (cap, _slot) = match unsafe { thread::with_current(|t| lookup_cap(t, cptr)) } {
        Ok(v) => v,
        Err(_) => return,
    };

    match cap.tag() {
        Some(CapTag::Endpoint) => {
            crate::api::ipc::send(uc, !nb, true);
        }
        Some(CapTag::Notification) => {
            if cap.notification_can_send() {
                let ntfn_ptr =
                    cap.notification_ptr() as *mut crate::object::notification::Notification;
                unsafe {
                    crate::object::notification::signal(ntfn_ptr, cap.notification_badge());
                }
            }
        }
        _ => do_send(uc, nb),
    }
}

fn read_t0(uc: &UserContext) -> u64 {
    uc.regs[UserRegister::T0.index()]
}

fn write_recv_cap_fault_or_empty(uc: &mut UserContext, cptr: u64) {
    if !crate::arch::riscv64::trap::send_cap_fault_ipc(uc, cptr, true) {
        write_empty(uc);
    }
}

fn write_empty(uc: &mut UserContext) {
    uc.regs[UserRegister::A0.index()] = 0;
    uc.regs[UserRegister::A1.index()] = 0;
    uc.regs[UserRegister::A2.index()] = 0;
    uc.regs[UserRegister::A3.index()] = 0;
    uc.regs[UserRegister::A4.index()] = 0;
    uc.regs[UserRegister::A5.index()] = 0;
    thread::zero_current_ipc_buffer_words(1, 4);
}
