//! Slow-path syscall dispatch (Call / Send / Recv / ReplyRecv …).
//!
//! For M3 we only handle `seL4_Call`, since that's what the rootserver
//! uses to drive cap invocations during bootstrap. The arch trap handler
//! decodes the syscall number and routes here; we read message registers
//! from the `UserContext`, perform the invocation, and write the reply
//! back into the same context before returning to user mode.

#![allow(dead_code)]

use crate::abi::types::MessageInfo;
use crate::api::cspace::lookup_cap;
use crate::api::invocation;
use crate::api::thread;
use crate::arch::riscv64::trap::{UserContext, reg};
use crate::object::cap::CapTag;

#[derive(Copy, Clone, Debug)]
pub enum SyscallError {
    InvalidArgument,
    InvalidCapability,
    IllegalOperation,
    RangeError,
    NotEnoughMemory,
    DeleteFirst,
    RevokeFirst,
    TruncatedMessage,
    FailedLookup,
    Unsupported,
    Preempted,
}

impl SyscallError {
    pub fn to_label(self) -> u64 {
        // seL4_Error from libsel4/include/sel4/errors.h:
        //   1 InvalidArgument, 2 InvalidCapability, 3 IllegalOperation,
        //   4 RangeError, 5 AlignmentError, 6 FailedLookup,
        //   7 TruncatedMessage, 8 DeleteFirst, 9 RevokeFirst, 10 NotEnoughMemory
        match self {
            Self::InvalidArgument => 1,
            Self::InvalidCapability => 2,
            Self::IllegalOperation => 3,
            Self::RangeError => 4,
            Self::FailedLookup => 6,
            Self::TruncatedMessage => 7,
            Self::DeleteFirst => 8,
            Self::RevokeFirst => 9,
            Self::NotEnoughMemory => 10,
            // No seL4_Error code for "not implemented" — use IllegalOperation.
            Self::Unsupported | Self::Preempted => 3,
        }
    }
}

/// Handle `seL4_Call`: cap lookup + invocation dispatch.
pub fn do_call(uc: &mut UserContext) {
    let cptr = uc.regs[reg::A0];
    let raw_info = uc.regs[reg::A1];
    let info = MessageInfo(raw_info);

    let t = unsafe { thread::current() };
    let lookup_res = lookup_cap(t, cptr);
    let (cap, slot) = match lookup_res {
        Ok(v) => v,
        Err(_) => {
            return write_error_reply(uc, SyscallError::InvalidCapability);
        }
    };

    let tag = cap.tag();
    let label = info.label();

    let result = match tag {
        Some(CapTag::Untyped) => invocation::handle_untyped(t, slot, cap, label, info.length(), uc),
        Some(CapTag::CNode) => invocation::handle_cnode(t, slot, cap, label, info.length(), uc),
        Some(CapTag::Frame) => invocation::handle_frame(t, slot, cap, label, info.length(), uc),
        Some(CapTag::PageTable) => {
            invocation::handle_page_table(t, slot, cap, label, info.length(), uc)
        }
        Some(CapTag::Thread) => invocation::handle_thread(t, slot, cap, label, info.length(), uc),
        Some(CapTag::Endpoint) => {
            // Real Send + implicit Reply via `api::ipc::call`. The
            // reply path lands here with a0/a1/MRs already populated
            // by `ipc::reply`, so we *don't* fall through to the
            // boilerplate `write_ok_reply` below — that would clobber
            // the reply.
            crate::api::ipc::call(uc);
            return;
        }
        Some(CapTag::Null) => Err(SyscallError::InvalidCapability),
        Some(CapTag::Domain) => invocation::handle_domain(t, cap, label, info.length(), uc),
        Some(CapTag::AsidControl) => {
            invocation::handle_asid_control(t, cap, label, info.length(), uc)
        }
        Some(CapTag::AsidPool) => invocation::handle_asid_pool(t, cap, label, info.length(), uc),
        Some(CapTag::IrqControl) => {
            invocation::handle_irq_control(t, slot, cap, label, info.length(), uc)
        }
        Some(CapTag::IrqHandler) => {
            invocation::handle_irq_handler(t, cap, label, info.length(), uc)
        }
        None => Err(SyscallError::InvalidCapability),
        _ => Err(SyscallError::IllegalOperation),
    };

    match result {
        Ok(()) => write_ok_reply(uc, 0, 0),
        Err(SyscallError::Preempted) => {}
        Err(e) => write_error_reply(uc, e),
    }
}

fn write_ok_reply(uc: &mut UserContext, label: u64, length: u64) {
    uc.regs[reg::A0] = 0; // badge
    uc.regs[reg::A1] = MessageInfo::new(label, 0, 0, length).0;
    // Don't touch a2..a5: leaving them as the user wrote matches the C
    // kernel's contract for "no extra reply mrs".
}

fn write_error_reply(uc: &mut UserContext, e: SyscallError) {
    // User code routinely invokes Call on caps that don't support the
    // requested label (e.g. SYSCALL0005 `seL4_Call` on the root CNode
    // cap); the error reply *is* the expected behaviour. Don't spam the
    // log — set the label and let the caller read it.
    uc.regs[reg::A0] = 0;
    uc.regs[reg::A1] = MessageInfo::new(e.to_label(), 0, 0, 0).0;
}

/// `seL4_Send` / `seL4_NBSend`: dispatch by cap type.
///
/// For Notification caps this becomes a `sendSignal` call. For
/// Endpoint caps we walk the EP state machine in `api::ipc::send`.
/// Other cap kinds (the test suite Sends to CNodes / Untyped during
/// SYSCALL0001/0002/0004) are silently dropped to match the C kernel.
pub fn do_send(uc: &mut UserContext, nb: bool) {
    let cptr = uc.regs[reg::A0];
    let t = unsafe { thread::current() };
    let (cap, slot) = match lookup_cap(t, cptr) {
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
            crate::api::ipc::send(uc, !nb);
        }
        Some(CapTag::Reply) => unsafe {
            let caller = cap.reply_tcb_ptr() as *mut crate::object::tcb::Tcb;
            crate::api::ipc::reply_to_tcb(uc, caller);
            if !caller.is_null() && (*caller).reply_slot == slot as u64 {
                (*caller).reply_slot = 0;
            }
            (*slot).cap = crate::object::cap::Cap::null();
        },
        _ => {}
    }
}

/// `seL4_Recv` / `seL4_NBRecv`: dispatch by cap type.
///
/// For Notification caps this becomes a `receiveSignal` (with poll
/// semantics: if no pending signal we just return badge=0 since we have
/// no scheduler to suspend on yet). For Endpoint caps we walk the EP
/// state machine in `api::ipc::recv`. Everything else: badge=0, msg=0.
pub fn do_recv(uc: &mut UserContext, blocking: bool) {
    let cptr = uc.regs[reg::A0];
    let t = unsafe { thread::current() };

    let cap_tag = match lookup_cap(t, cptr) {
        Ok((cap, _slot)) => cap.tag(),
        Err(_) => None,
    };

    match cap_tag {
        Some(CapTag::Endpoint) => crate::api::ipc::recv(uc, blocking),
        Some(CapTag::Notification) => {
            let (cap, _slot) = lookup_cap(t, cptr).expect("recap");
            if cap.notification_can_receive() {
                let ntfn_ptr =
                    cap.notification_ptr() as *mut crate::object::notification::Notification;
                let cur_tcb = crate::object::tcb::current();
                let outcome =
                    unsafe { crate::object::notification::wait(ntfn_ptr, cur_tcb, blocking) };
                match outcome {
                    crate::object::notification::WaitOutcome::Got(badge) => {
                        uc.regs[reg::A0] = badge;
                        uc.regs[reg::A1] = 0;
                        if !t.ipc_buffer_kva.is_null() {
                            unsafe {
                                for i in 0..4 {
                                    *t.ipc_buffer_kva.add(1 + i) = 0;
                                }
                            }
                        }
                        uc.regs[reg::A2] = 0;
                        uc.regs[reg::A3] = 0;
                        uc.regs[reg::A4] = 0;
                        uc.regs[reg::A5] = 0;
                    }
                    crate::object::notification::WaitOutcome::Blocked => {
                        // Caller is now BlockedOnNotification — leave its
                        // registers alone; signal() will write them at
                        // wake-up time. `kernel_exit` will pick another
                        // runnable TCB.
                    }
                }
            } else {
                write_empty(uc);
            }
        }
        _ => write_empty(uc),
    }
}

fn write_empty(uc: &mut UserContext) {
    uc.regs[reg::A0] = 0;
    uc.regs[reg::A1] = 0;
    uc.regs[reg::A2] = 0;
    uc.regs[reg::A3] = 0;
    uc.regs[reg::A4] = 0;
    uc.regs[reg::A5] = 0;
    let t = unsafe { thread::current() };
    if !t.ipc_buffer_kva.is_null() {
        unsafe {
            for i in 0..4 {
                *t.ipc_buffer_kva.add(1 + i) = 0;
            }
        }
    }
}
