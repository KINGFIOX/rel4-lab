//! Slow-path syscall dispatch (Call / Send / Recv / ReplyRecv …).
//!
//! The arch trap handler decodes the syscall number and routes here. This
//! module dispatches capability invocations, endpoint/notification IPC,
//! replies, and explicit-reply receive variants, then writes the seL4-style reply
//! registers back into the saved `UserContext` before returning to user mode.

#![allow(dead_code)]

use crate::abi::constants::N_MSG_REGISTERS;
use crate::abi::types::MessageInfo;
use crate::api::cspace::lookup_cap;
use crate::api::invocation;
use crate::api::thread;
use crate::arch::current::api::UserContext;
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
    let cptr = uc.cap_reg();
    let raw_info = uc.msg_info();
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
                #[cfg(target_arch = "x86_64")]
                Some(CapTag::IoPortControl) => {
                    invocation::handle_io_port_control(t, slot, cap, label, length, uc)
                }
                #[cfg(target_arch = "x86_64")]
                Some(CapTag::IoPort) => invocation::handle_io_port(t, cap, label, length, uc),
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
        // Endpoint Call is a real IPC send. A successful reply will later
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
    if crate::object::tcb::runnable_snapshot(current) {
        crate::arch::current::api::apply_preemption_restart(uc);
    }
}

fn write_ok_reply(uc: &mut UserContext, label: u64, length: u64) {
    uc.set_cap_reg(0); // badge
    uc.set_msg_info(MessageInfo::new(label, 0, 0, length).0);
    // Don't touch a2..a5: leaving them as the user wrote matches the C
    // kernel's contract for "no extra reply mrs".
}

fn write_error_reply(uc: &mut UserContext, e: SyscallError) {
    // User code routinely invokes Call on caps that don't support the
    // requested label (e.g. SYSCALL0005 `seL4_Call` on the root CNode
    // cap); the error reply *is* the expected behaviour. Don't spam the
    // log — set the label and let the caller read it.
    uc.set_cap_reg(0);
    uc.set_msg_info(MessageInfo::new(e.to_label(), 0, 0, 0).0);
}

/// `seL4_Send` / `seL4_NBSend`: dispatch by cap type.
///
/// For Notification caps this becomes a `sendSignal` call. For
/// Endpoint caps we walk the EP state machine in `api::ipc::send`.
/// Thread caps support the FPU-related `TCB_SetFlags` send-only
/// invocation. Other cap kinds (the test suite Sends to CNodes / Untyped
/// during SYSCALL0001/0002/0004) are silently dropped to match the local
/// compatibility baseline.
pub fn do_send(uc: &mut UserContext, nb: bool) {
    let cptr = uc.cap_reg();
    let raw_info = uc.msg_info();
    let info = MessageInfo(raw_info);
    let label = info.label();
    let mut length = info.length();
    if length > N_MSG_REGISTERS as u64 && !thread::current_has_ipc_buffer() {
        length = N_MSG_REGISTERS as u64;
    }
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
            crate::api::ipc::handle_reply_slot(uc, (cap, slot));
        },
        Some(CapTag::Thread) => unsafe {
            thread::with_current(|t| {
                let _ = invocation::handle_thread_send(t, slot, cap, label, length, uc);
            });
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
    do_recv_inner(uc, blocking)
}

fn do_recv_inner(uc: &mut UserContext, blocking: bool) {
    unsafe {
        crate::object::reply::delete_caller_cap(crate::object::tcb::current());
    }
    let cptr = uc.cap_reg();
    let cap = match unsafe { thread::with_current(|t| lookup_cap(t, cptr)) } {
        Ok((cap, _slot)) => cap,
        Err(_) => {
            write_recv_cap_fault_or_empty(uc, cptr);
            return;
        }
    };

    match cap.tag() {
        Some(CapTag::Endpoint) => {
            crate::api::ipc::recv(uc, blocking);
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
                    uc.set_cap_reg(badge);
                    uc.set_msg_info(0);
                    thread::zero_current_ipc_buffer_words(1, 4);
                    uc.set_mr(0, 0);
                    uc.set_mr(1, 0);
                    uc.set_mr(2, 0);
                    uc.set_mr(3, 0);
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

fn write_recv_cap_fault_or_empty(uc: &mut UserContext, cptr: u64) {
    if !crate::arch::current::api::send_cap_fault_ipc(uc, cptr, true) {
        write_empty(uc);
    }
}

fn write_empty(uc: &mut UserContext) {
    uc.set_cap_reg(0);
    uc.set_msg_info(0);
    uc.set_mr(0, 0);
    uc.set_mr(1, 0);
    uc.set_mr(2, 0);
    uc.set_mr(3, 0);
    thread::zero_current_ipc_buffer_words(1, 4);
}
