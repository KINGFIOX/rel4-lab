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
use crate::arch::riscv64::trap::{reg, UserContext};
use crate::object::cap::{Cap, CapTag};
use crate::object::endpoint::{self, EpState};
use crate::object::tcb::{self, ThreadState};

/// `seL4_MsgMaxLength` (libsel4/include/sel4/constants.h).
const MSG_MAX_LENGTH: u64 = 120;
const MR_REG_COUNT: u64 = 4;

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
unsafe fn block_sender(ep: *mut endpoint::Endpoint, is_call: bool, badge: u64) {
    let cur = tcb::current();
    if cur.is_null() {
        return;
    }
    unsafe {
        tcb::dequeue(cur);
        (*cur).state = ThreadState::BlockedOnSend as u8;
        (*cur).waiting_on = ep as u64;
        (*cur).sender_badge = badge;
        (*cur).sender_is_call = if is_call { 1 } else { 0 };
        endpoint::enqueue_waiter(ep, cur, EpState::Sending);
    }
}

/// Block the current TCB on `ep` as a receiver. No payload to stash.
unsafe fn block_receiver(ep: *mut endpoint::Endpoint) {
    let cur = tcb::current();
    if cur.is_null() {
        return;
    }
    unsafe {
        tcb::dequeue(cur);
        (*cur).state = ThreadState::BlockedOnReceive as u8;
        (*cur).waiting_on = ep as u64;
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
                    unsafe { block_sender(ep, false, badge) };
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
            unsafe { block_sender(ep, false, badge) };
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
        write_empty_reply(uc);
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
                    unsafe { block_receiver(ep) };
                } else {
                    write_empty_reply(uc);
                }
                return;
            }
            let info_in = unsafe { MessageInfo((*sender).context.regs[reg::A1]) };
            let badge = unsafe { (*sender).sender_badge };
            let is_call = unsafe { (*sender).sender_is_call } != 0;
            unsafe {
                transfer_message(sender, cur, info_in, badge);
                (*sender).waiting_on = 0;
                if is_call {
                    // Park the caller on Reply; record the reply
                    // target on the receiver so seL4_Reply later
                    // wakes the right TCB.
                    (*sender).state = ThreadState::BlockedOnReply as u8;
                    (*sender).sender_is_call = 0;
                    (*cur).caller = sender as u64;
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
                    let n =
                        bound as *mut crate::object::notification::Notification;
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
                unsafe { block_receiver(ep) };
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
                unsafe { block_sender(ep, true, badge) };
                return;
            }
            unsafe {
                transfer_message(cur, receiver, info, badge);
                (*receiver).waiting_on = 0;
                (*receiver).caller = cur as u64;
                (*receiver).state = ThreadState::Running as u8;
                tcb::enqueue(receiver);
                // Park the caller until Reply comes back.
                tcb::dequeue(cur);
                (*cur).state = ThreadState::BlockedOnReply as u8;
                (*cur).waiting_on = 0;
            }
        }
        EpState::Idle | EpState::Sending => {
            unsafe { block_sender(ep, true, badge) };
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
    let info = MessageInfo(uc.regs[reg::A1]);
    unsafe {
        transfer_message(cur, caller, info, 0);
        (*caller).state = ThreadState::Running as u8;
        (*caller).waiting_on = 0;
        tcb::enqueue(caller);
        (*cur).caller = 0;
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
