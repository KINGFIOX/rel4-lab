//! Endpoint-IPC core: message transfer plus the Send / Recv / Call /
//! Reply / ReplyRecv state-machine glue that bridges `syscall::do_*`
//! to `object::endpoint`.
//!
//! Design follows the C kernel's pre-MCS path:
//!
//! * `seL4_Send` (blocking) and `seL4_Call` walk the EP. If a receiver
//!   is already waiting → rendezvous, transfer message, wake the
//!   receiver (Call additionally parks the sender on `BlockedOnReply`
//!   and stamps `receiver.caller = sender`). Otherwise the sender is
//!   dequeued from the runqueue and queued on the EP (`BlockedOnSend`).
//! * `seL4_NBSend` is the Send path minus the queueing fallback —
//!   no receiver waiting means the message is dropped.
//! * `seL4_Recv` (blocking) walks the EP for a queued sender, and if
//!   one is found rendezvous + wake-or-park-the-sender (depending on
//!   whether the queued sender was a Call). No sender → block on the
//!   EP (`BlockedOnReceive`).
//! * `seL4_NBRecv` is Recv minus the blocking fallback.
//! * `seL4_Reply` walks the current TCB's `caller`. If non-null it
//!   transfers the reply payload and wakes the caller; otherwise the
//!   call is a no-op (matches `pre-MCS` C kernel: a Reply with no
//!   caller silently drops).
//! * `seL4_ReplyRecv` is Reply followed by Recv on a fresh cap.
//!
//! Message-register transfer:
//!   * MR[0..3] live in regs[A2..A5] — copied register-to-register.
//!   * MR[4..length] live in the sender's IPC buffer at words [1+i] —
//!     copied to the receiver's IPC buffer at the same offset.
//!   * The receiver's a0 is set to the *sender's* badge (from the
//!     sender's cap, stashed in `sender_badge` when queueing).
//!
//! Notes / non-goals for this iteration:
//!   * `seL4_MessageInfo.caps_unwrapped` and cap transfer aren't yet
//!     implemented — sel4test exercises plain message-register IPC.
//!   * No cross-VSpace context switch: kernel_exit still runs every
//!     TCB on the rootserver's PT (helpers share it). Multi-VSpace
//!     switching is the next iteration.

#![allow(dead_code)]

use crate::abi::types::MessageInfo;
use crate::api::cspace::lookup_cap;
use crate::api::thread;
use crate::arch::riscv64::trap::{UserContext, reg};
use crate::object::cap::{Cap, CapTag};
use crate::object::endpoint::{self, EpState};
use crate::object::tcb::{self, ThreadState};

/// `seL4_MsgMaxLength` (libsel4/include/sel4/constants.h).
const MSG_MAX_LENGTH: u64 = 120;
const MR_REG_COUNT: u64 = 4;
const FAULT_UNKNOWN_SYSCALL: u64 = 2;
const FAULT_USER_EXCEPTION: u64 = 3;

/// Copy MRs from `sender` into `receiver`, set the receiver's badge +
/// reply MessageInfo. `length` is the truncated MR count to deliver.
unsafe fn transfer_message(
    sender: *mut tcb::Tcb,
    receiver: *mut tcb::Tcb,
    info_in: MessageInfo,
    badge: u64,
) {
    let label = info_in.label();
    let mut length = info_in.length();
    if length > MSG_MAX_LENGTH {
        length = MSG_MAX_LENGTH;
    }
    let info_out = MessageInfo::new(label, 0, 0, length);

    unsafe {
        (*receiver).context.regs[reg::A0] = badge;
        (*receiver).context.regs[reg::A1] = info_out.0;

        let mr_reg_n = length.min(MR_REG_COUNT) as usize;
        for i in 0..mr_reg_n {
            (*receiver).context.regs[reg::A2 + i] = (*sender).context.regs[reg::A2 + i];
        }

        if length > MR_REG_COUNT {
            let sbuf = (*sender).ipc_buffer_kva;
            let rbuf = (*receiver).ipc_buffer_kva;
            if sbuf != 0 && rbuf != 0 {
                let sbuf = sbuf as *const u64;
                let rbuf = rbuf as *mut u64;
                let extra = length - MR_REG_COUNT;
                for i in 0..extra as usize {
                    let off = 1 + MR_REG_COUNT as usize + i;
                    let v = *sbuf.add(off);
                    *rbuf.add(off) = v;
                }
            }
        }
    }
}

unsafe fn transfer_fault_message(sender: *mut tcb::Tcb, receiver: *mut tcb::Tcb, badge: u64) {
    let mut length = unsafe { (*sender).fault_len };
    if length > MSG_MAX_LENGTH {
        length = MSG_MAX_LENGTH;
    }
    let info_out = unsafe { MessageInfo::new((*sender).fault_label, 0, 0, length) };

    unsafe {
        (*receiver).context.regs[reg::A0] = badge;
        (*receiver).context.regs[reg::A1] = info_out.0;

        let mr_reg_n = length.min(MR_REG_COUNT) as usize;
        for i in 0..mr_reg_n {
            (*receiver).context.regs[reg::A2 + i] = (*sender).fault_mrs[i];
        }

        if length > MR_REG_COUNT {
            let rbuf = (*receiver).ipc_buffer_kva;
            if rbuf != 0 {
                let rbuf = rbuf as *mut u64;
                let extra = length - MR_REG_COUNT;
                for i in 0..extra as usize {
                    let off = 1 + MR_REG_COUNT as usize + i;
                    *rbuf.add(off) = (*sender).fault_mrs[MR_REG_COUNT as usize + i];
                }
            }
        }
    }
}

/// Look up the cap and badge / permission bits for an Endpoint
/// reference at `cptr`. Returns `None` if the cap is missing or not
/// an Endpoint.
fn lookup_endpoint(cptr: u64) -> Option<(Cap, *mut endpoint::Endpoint, u64)> {
    let t = unsafe { thread::current() };
    let (cap, _slot) = lookup_cap(t, cptr).ok()?;
    match cap.tag()? {
        CapTag::Endpoint => {
            let ep = cap.endpoint_ptr() as *mut endpoint::Endpoint;
            let badge = cap.endpoint_badge();
            Some((cap, ep, badge))
        }
        _ => None,
    }
}

/// Block the current TCB on `ep` as a sender. Caller stashes the cap
/// badge / "is this a Call?" bit so the rendezvous logic can deliver
/// the right semantics.
unsafe fn block_sender(
    ep: *mut endpoint::Endpoint,
    is_call: bool,
    badge: u64,
    can_grant: bool,
    can_grant_reply: bool,
) {
    let cur = tcb::current();
    if cur.is_null() {
        return;
    }
    unsafe {
        tcb::dequeue(cur);
        (*cur).state = ThreadState::BlockedOnSend as u8;
        (*cur).waiting_on = ep as u64;
        (*cur).sender_badge = badge;
        (*cur).sender_can_grant = if can_grant { 1 } else { 0 };
        (*cur).sender_can_grant_reply = if can_grant_reply { 1 } else { 0 };
        (*cur).sender_is_call = if is_call { 1 } else { 0 };
        (*cur).sender_is_fault = 0;
        endpoint::enqueue_waiter(ep, cur, EpState::Sending);
    }
}

/// Block the current TCB on `ep` as a receiver. No payload to stash.
unsafe fn block_receiver(ep: *mut endpoint::Endpoint, can_grant: bool) {
    let cur = tcb::current();
    if cur.is_null() {
        return;
    }
    unsafe {
        tcb::dequeue(cur);
        (*cur).state = ThreadState::BlockedOnReceive as u8;
        (*cur).waiting_on = ep as u64;
        (*cur).receiver_can_grant = if can_grant { 1 } else { 0 };
        endpoint::enqueue_waiter(ep, cur, EpState::Receiving);
    }
}

/// `seL4_Send` on an Endpoint. `blocking` controls whether we queue
/// (true → `seL4_Send`) or drop (false → `seL4_NBSend`) when no
/// receiver is waiting.
pub fn send(uc: &mut UserContext, blocking: bool) {
    let cptr = uc.regs[reg::A0];
    let info = MessageInfo(uc.regs[reg::A1]);

    let (cap, ep, badge) = match lookup_endpoint(cptr) {
        Some(v) => v,
        None => return,
    };
    if !cap.endpoint_can_send() {
        return;
    }

    let cur = tcb::current();
    if cur.is_null() {
        return;
    }
    let ep_state = unsafe { (*ep).state() };
    match ep_state {
        EpState::Receiving => {
            let receiver = unsafe { endpoint::pop_head(ep) };
            if receiver.is_null() {
                if blocking {
                    unsafe {
                        block_sender(
                            ep,
                            false,
                            badge,
                            cap.endpoint_can_grant(),
                            cap.endpoint_can_grant_reply(),
                        )
                    };
                }
                return;
            }
            unsafe {
                transfer_message(cur, receiver, info, badge);
                (*receiver).waiting_on = 0;
                (*receiver).state = ThreadState::Running as u8;
                tcb::enqueue(receiver);
            }
        }
        EpState::Idle | EpState::Sending => {
            if !blocking {
                return;
            }
            unsafe {
                block_sender(
                    ep,
                    false,
                    badge,
                    cap.endpoint_can_grant(),
                    cap.endpoint_can_grant_reply(),
                )
            };
        }
    }
}

/// `seL4_Recv` on an Endpoint. Returns synthesised reply (badge=0,
/// label=0, length=0) if no sender is waiting and `blocking=false`.
pub fn recv(uc: &mut UserContext, blocking: bool) {
    let cptr = uc.regs[reg::A0];

    let (cap, ep, _) = match lookup_endpoint(cptr) {
        Some(v) => v,
        None => {
            write_empty_reply(uc);
            return;
        }
    };
    if !cap.endpoint_can_receive() {
        if !crate::arch::riscv64::trap::send_cap_fault_ipc(uc, cptr, true) {
            write_empty_reply(uc);
        }
        return;
    }

    let cur = tcb::current();
    if cur.is_null() {
        write_empty_reply(uc);
        return;
    }
    let ep_state = unsafe { (*ep).state() };
    match ep_state {
        EpState::Sending => {
            let sender = unsafe { endpoint::pop_head(ep) };
            if sender.is_null() {
                if blocking {
                    unsafe { block_receiver(ep, cap.endpoint_can_grant()) };
                } else {
                    write_empty_reply(uc);
                }
                return;
            }
            let info_in = unsafe { MessageInfo((*sender).context.regs[reg::A1]) };
            let badge = unsafe { (*sender).sender_badge };
            let is_call = unsafe { (*sender).sender_is_call } != 0;
            let can_reply = unsafe {
                (*sender).sender_can_grant != 0 || (*sender).sender_can_grant_reply != 0
            };
            let is_fault = unsafe { (*sender).sender_is_fault } != 0;
            unsafe {
                if is_fault {
                    transfer_fault_message(sender, cur, badge);
                } else {
                    transfer_message(sender, cur, info_in, badge);
                }
                (*sender).waiting_on = 0;
                if is_call && can_reply {
                    // Park the caller on Reply; record the reply
                    // target on the receiver so seL4_Reply later
                    // wakes the right TCB.
                    (*sender).state = ThreadState::BlockedOnReply as u8;
                    (*sender).sender_is_call = 0;
                    (*cur).caller = sender as u64;
                    (*cur).caller_can_grant = cap.endpoint_can_grant() as u8;
                } else if is_call {
                    (*sender).context.pc = (*sender).context.pc.wrapping_sub(4);
                    (*sender).state = ThreadState::Inactive as u8;
                    (*sender).sender_is_call = 0;
                } else {
                    // Plain Send: wake the sender, drop its badge.
                    (*sender).state = ThreadState::Running as u8;
                    tcb::enqueue(sender);
                }
            }
        }
        EpState::Idle | EpState::Receiving => {
            // Before blocking on the Endpoint, check the bound
            // Notification. The C kernel's `receiveIPC` path does the
            // same when the TCB has a bound ntfn that's Active: it
            // returns the notification's badge as the IPC reply
            // instead of queuing on the EP. This is what lets
            // BIND0001 deliver ASYNC signals through a `seL4_Wait` on
            // a *different* (sync) endpoint.
            unsafe {
                let bound = (*cur).bound_notification;
                if bound != 0 {
                    let n = bound as *mut crate::object::notification::Notification;
                    if (*n).state() == crate::object::notification::NtfnState::Active {
                        let badge = (*n).badge();
                        (*n).set_badge(0);
                        (*n).set_state(crate::object::notification::NtfnState::Idle);
                        uc.regs[reg::A0] = badge;
                        uc.regs[reg::A1] = 0;
                        uc.regs[reg::A2] = 0;
                        uc.regs[reg::A3] = 0;
                        uc.regs[reg::A4] = 0;
                        uc.regs[reg::A5] = 0;
                        // Mirror MR[0..3] into the buffer too, mirroring
                        // write_empty_reply.
                        let buf = (*cur).ipc_buffer_kva;
                        if buf != 0 {
                            let p = buf as *mut u64;
                            for i in 0..MR_REG_COUNT as usize {
                                *p.add(1 + i) = 0;
                            }
                        }
                        return;
                    }
                }
            }
            if blocking {
                unsafe { block_receiver(ep, cap.endpoint_can_grant()) };
            } else {
                write_empty_reply(uc);
            }
        }
    }
}

/// `seL4_Call`. Equivalent to a blocking Send followed by an implicit
/// wait for the matching Reply. Rendezvous transfers the message,
/// records `receiver.caller = current`, and parks the caller on
/// `BlockedOnReply`. No receiver waiting → queue as a Call sender.
pub fn call(uc: &mut UserContext) {
    let cptr = uc.regs[reg::A0];
    let info = MessageInfo(uc.regs[reg::A1]);

    let (cap, ep, badge) = match lookup_endpoint(cptr) {
        Some(v) => v,
        None => return, // syscall.rs falls back to its existing handler
    };
    if !cap.endpoint_can_send() {
        return;
    }

    let cur = tcb::current();
    if cur.is_null() {
        return;
    }
    let ep_state = unsafe { (*ep).state() };
    match ep_state {
        EpState::Receiving => {
            let receiver = unsafe { endpoint::pop_head(ep) };
            if receiver.is_null() {
                // Queue saw a receiver but pop returned null — treat
                // as no-receiver and queue the caller.
                unsafe {
                    block_sender(
                        ep,
                        true,
                        badge,
                        cap.endpoint_can_grant(),
                        cap.endpoint_can_grant_reply(),
                    )
                };
                return;
            }
            unsafe {
                transfer_message(cur, receiver, info, badge);
                (*receiver).waiting_on = 0;
                (*receiver).state = ThreadState::Running as u8;
                tcb::enqueue(receiver);
                tcb::dequeue(cur);
                if cap.endpoint_can_grant() || cap.endpoint_can_grant_reply() {
                    (*receiver).caller = cur as u64;
                    (*receiver).caller_can_grant = (*receiver).receiver_can_grant;
                    // Park the caller until Reply comes back.
                    (*cur).state = ThreadState::BlockedOnReply as u8;
                } else {
                    (*cur).context.pc = (*cur).context.pc.wrapping_sub(4);
                    (*cur).state = ThreadState::Inactive as u8;
                }
                (*cur).waiting_on = 0;
                (*cur).sender_is_fault = 0;
            }
        }
        EpState::Idle | EpState::Sending => {
            unsafe {
                block_sender(
                    ep,
                    true,
                    badge,
                    cap.endpoint_can_grant(),
                    cap.endpoint_can_grant_reply(),
                )
            };
        }
    }
}

/// `seL4_Reply`. Walks the current TCB's `caller`, transfers the
/// reply payload, and wakes the caller. Returns silently if no
/// caller is recorded (matches the pre-MCS C kernel).
pub fn reply(uc: &mut UserContext) {
    let cur = tcb::current();
    if cur.is_null() {
        return;
    }
    let caller = unsafe { (*cur).caller } as *mut tcb::Tcb;
    if caller.is_null() {
        return;
    }
    unsafe { reply_to_tcb(uc, caller) };
    unsafe {
        (*cur).caller = 0;
        (*cur).caller_can_grant = 0;
    }
}

pub unsafe fn reply_to_tcb(uc: &mut UserContext, caller: *mut tcb::Tcb) {
    if caller.is_null() {
        return;
    }
    let cur = tcb::current();
    if cur.is_null() {
        return;
    }
    let info = MessageInfo(uc.regs[reg::A1]);
    unsafe {
        let mut wake_caller = true;
        if (*caller).sender_is_fault == 0 {
            transfer_message(cur, caller, info, 0);
        } else {
            if info.label() == 0 {
                match (*caller).fault_label {
                    FAULT_UNKNOWN_SYSCALL => apply_unknown_syscall_reply(cur, uc, caller),
                    FAULT_USER_EXCEPTION => {
                        (*caller).context.pc = reply_mr(cur, uc, 0);
                        (*caller).context.regs[reg::SP] = reply_mr(cur, uc, 1);
                    }
                    _ => {}
                }
            } else if (*caller).fault_label == FAULT_UNKNOWN_SYSCALL
                || (*caller).fault_label == FAULT_USER_EXCEPTION
            {
                wake_caller = false;
            }
            (*caller).sender_is_fault = 0;
            (*caller).fault_label = 0;
            (*caller).fault_len = 0;
            (*caller).fault_mrs = [0; 16];
        }
        (*caller).waiting_on = 0;
        if wake_caller {
            (*caller).state = ThreadState::Running as u8;
            tcb::enqueue(caller);
        } else {
            (*caller).state = ThreadState::Inactive as u8;
        }
    }
}

unsafe fn reply_mr(sender: *mut tcb::Tcb, uc: &UserContext, i: usize) -> u64 {
    match i {
        0 => uc.regs[reg::A2],
        1 => uc.regs[reg::A3],
        2 => uc.regs[reg::A4],
        3 => uc.regs[reg::A5],
        _ => unsafe {
            let buf = (*sender).ipc_buffer_kva;
            if buf == 0 {
                0
            } else {
                *((buf as *const u64).add(1 + i))
            }
        },
    }
}

unsafe fn apply_unknown_syscall_reply(
    sender: *mut tcb::Tcb,
    uc: &UserContext,
    caller: *mut tcb::Tcb,
) {
    unsafe {
        (*caller).context.pc = reply_mr(sender, uc, 0);
        (*caller).context.regs[reg::SP] = reply_mr(sender, uc, 1);
        (*caller).context.regs[reg::RA] = reply_mr(sender, uc, 2);
        (*caller).context.regs[reg::A0] = reply_mr(sender, uc, 3);
        (*caller).context.regs[reg::A1] = reply_mr(sender, uc, 4);
        (*caller).context.regs[reg::A2] = reply_mr(sender, uc, 5);
        (*caller).context.regs[reg::A3] = reply_mr(sender, uc, 6);
        (*caller).context.regs[reg::A4] = reply_mr(sender, uc, 7);
        (*caller).context.regs[reg::A5] = reply_mr(sender, uc, 8);
        (*caller).context.regs[reg::A6] = reply_mr(sender, uc, 9);
        (*caller).context.regs[reg::A7] = reply_mr(sender, uc, 10);
    }
}

/// `seL4_ReplyRecv`: reply to the implicit caller, then immediately
/// Recv on the supplied EP cap. Used by classic seL4 servers.
pub fn reply_recv(uc: &mut UserContext) {
    reply(uc);
    recv(uc, true);
}

/// "No sender, no payload" reply written into the syscall return
/// registers. Used by `recv` when there's nothing pending and the
/// caller asked for non-blocking semantics (or the cap was bogus).
/// Mirrors the M3 stubbed behaviour so userspace doesn't see leftover
/// register state from the trap path.
fn write_empty_reply(uc: &mut UserContext) {
    uc.regs[reg::A0] = 0;
    uc.regs[reg::A1] = 0;
    uc.regs[reg::A2] = 0;
    uc.regs[reg::A3] = 0;
    uc.regs[reg::A4] = 0;
    uc.regs[reg::A5] = 0;
    // Clear MR[0..3] in the IPC buffer too so seL4_GetMR sees zeros.
    let t = unsafe { thread::current() };
    if !t.ipc_buffer_kva.is_null() {
        unsafe {
            for i in 0..MR_REG_COUNT as usize {
                *t.ipc_buffer_kva.add(1 + i) = 0;
            }
        }
    }
}
