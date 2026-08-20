//! Endpoint-IPC core: message transfer plus the Send / Recv / Call /
//! Reply / ReplyRecv state-machine glue that bridges `syscall::do_*`
//! to `object::endpoint`.
//!
//! Design follows non-MCS seL4 IPC:
//!
//! * `seL4_Send` (blocking) and `seL4_Call` walk the EP. If a receiver
//!   is already waiting → rendezvous, transfer message, wake the
//!   receiver. `Call` parks the sender on `BlockedOnReply` and inserts a
//!   derived reply cap into the receiver's `tcbCaller` slot. If no
//!   receiver is waiting, the sender is queued on the EP (`BlockedOnSend`).
//! * `seL4_NBSend` is the Send path minus the queueing fallback —
//!   no receiver waiting means the message is dropped.
//! * `seL4_Recv` (blocking) first deletes any leftover caller cap, then
//!   walks the EP for a queued sender. No sender → block on the EP
//!   (`BlockedOnReceive`).
//! * `seL4_NBRecv` is Recv minus the blocking fallback.
//! * `seL4_Reply` reads the current thread's `tcbCaller` slot.
//! * `seL4_ReplyRecv` is Reply followed by Recv.
//!
//! Message-register transfer:
//!   * MR[0..3] live in regs[A2..A5] — copied register-to-register.
//!   * MR[4..length] live in the sender's IPC buffer at words [1+i] —
//!     copied to the receiver's IPC buffer at the same offset.
//!   * The receiver's a0 is set to the *sender's* badge (from the
//!     sender's cap, stashed in `sender_badge` when queueing).
//!
//! seL4 alignment notes:
//!   * Cap transfer follows upstream seL4's `transferCaps` /
//!     `getReceiveSlots` model: the sender can name up to
//!     `seL4_MsgMaxExtraCaps` extra caps, but there is only one destination
//!     receive slot for an inserted cap. Endpoint caps to the send endpoint
//!     are unwrapped into badges instead of inserted.
//!   * VSpace switching is handled on kernel exit from each TCB's VTable
//!     CTE slot; this layer only performs the IPC object and message-transfer
//!     state transitions.

#![allow(dead_code)]
// This module is written entirely in terms of the safe abstractions in
// `ktypes`; keep it that way.
#![deny(unsafe_code)]

use crate::abi::fault::FaultLabel;
use crate::abi::types::MessageInfo;
use crate::api::cspace::{self, lookup_cap_current};
use crate::api::invocation::derive_cap_for_copy;
use crate::arch::current::api::UserContext;
use crate::arch::current::sel4_arch::{
    UNKNOWN_SYSCALL_FAULT_IP_MR, UNKNOWN_SYSCALL_REPLY_REGS, USER_EXCEPTION_SP_REG,
};
use crate::object::cap::{Cap, CapTag};
use crate::object::cnode::CteRef;
use crate::object::endpoint::{EndpointRef, EpState};
use crate::object::tcb::{self, IpcBuffer, TcbRef};

/// `seL4_MsgMaxLength` (libsel4/include/sel4/constants.h).
const MSG_MAX_LENGTH: u64 = 120;
const MSG_MAX_EXTRA_CAPS: u64 = 3;
const MSG_MAX_EXTRA_CAPS_USIZE: usize = MSG_MAX_EXTRA_CAPS as usize;
const MR_REG_COUNT: u64 = 4;
const MR_REG_COUNT_USIZE: usize = MR_REG_COUNT as usize;
type ExtraCapSlots = [Option<CteRef>; MSG_MAX_EXTRA_CAPS_USIZE];
const NO_EXTRA_CAPS: ExtraCapSlots = [None; MSG_MAX_EXTRA_CAPS_USIZE];

#[repr(usize)]
#[derive(Copy, Clone, Debug, Eq, PartialEq)]
enum IpcBufferSlot {
    CapsOrBadges = 122,
    ReceiveCNode = 125,
    ReceiveIndex = 126,
    ReceiveDepth = 127,
}

impl IpcBufferSlot {
    const fn index(self) -> usize {
        self as usize
    }
}

/// Copy MRs from `sender` into `receiver`, set the receiver's badge and
/// reply MessageInfo. `length` is the truncated MR count to deliver.
fn transfer_message(
    sender: TcbRef,
    receiver: TcbRef,
    info_in: MessageInfo,
    badge: u64,
    endpoint: Option<EndpointRef>,
    can_grant: bool,
    extra_cap_slots: ExtraCapSlots,
) {
    let label = info_in.label();
    let length = info_in.length().min(MSG_MAX_LENGTH);

    let mr_regs = sender.ipc_message_regs(length);
    receiver.write_ipc_message_regs(badge, &mr_regs, length);

    let mut transferred_length = length;
    if length > MR_REG_COUNT {
        match (sender.ipc_buffer(), receiver.ipc_buffer()) {
            (Some(from), Some(to)) => {
                let extra = (length - MR_REG_COUNT) as usize;
                to.copy_words_from(from, 1 + MR_REG_COUNT_USIZE, extra);
            }
            _ => transferred_length = MR_REG_COUNT,
        }
    }

    let (caps_unwrapped, extra_caps) =
        transfer_caps(receiver, info_in, endpoint, can_grant, extra_cap_slots);
    let info_out = MessageInfo::new(label, caps_unwrapped, extra_caps, transferred_length);
    receiver.write_message_info(info_out.0);
}

fn transfer_caps(
    receiver: TcbRef,
    info_in: MessageInfo,
    endpoint: Option<EndpointRef>,
    can_grant: bool,
    extra_cap_slots: ExtraCapSlots,
) -> (u64, u64) {
    // Mirrors upstream seL4's single-cap receive-slot transfer. `extraCaps`
    // reports how many extra cap refs were consumed, not how many caps were
    // inserted; endpoint unwraps count as consumed refs.
    if !can_grant {
        return (0, 0);
    }
    let requested = info_in.extra_caps().min(MSG_MAX_EXTRA_CAPS);
    if requested == 0 || extra_cap_slots[0].is_none() {
        return (0, 0);
    }
    let Some(receiver_buffer) = receiver.ipc_buffer() else {
        return (0, 0);
    };

    let mut dest_slot = get_receive_slot(receiver);
    let mut caps_unwrapped = 0u64;
    let mut transferred = 0u64;

    for i in 0..requested as usize {
        let Some(src_slot) = extra_cap_slots[i] else {
            break;
        };
        let src_cap = src_slot.cap();
        if src_cap.is_null() {
            break;
        }

        // A cap to the endpoint the message came in on is unwrapped into its
        // badge rather than inserted into the receiver's CSpace.
        if endpoint.is_some() && src_cap.as_endpoint() == endpoint {
            if !receiver_buffer.set_word(
                IpcBufferSlot::CapsOrBadges.index() + i,
                src_cap.endpoint_badge(),
            ) {
                break;
            }
            caps_unwrapped |= 1u64 << i;
            transferred = i as u64 + 1;
            continue;
        }

        let Some(dst) = dest_slot else {
            break;
        };
        let derived = match derive_cap_for_copy(src_slot, src_cap) {
            Ok(cap) if !cap.is_null() => cap,
            _ => break,
        };
        if !dst.is_empty() {
            break;
        }
        src_slot.cte_insert(derived, dst);
        dest_slot = None;

        transferred = i as u64 + 1;
    }

    (caps_unwrapped, transferred)
}

/// Resolve the cptrs the sender named as extra caps, before the rendezvous
/// changes either thread's CSpace view.
fn snapshot_extra_cap_slots(
    sender: TcbRef,
    info: MessageInfo,
    can_grant: bool,
) -> Result<ExtraCapSlots, u64> {
    let mut slots = NO_EXTRA_CAPS;
    if !can_grant {
        return Ok(slots);
    }
    let requested = info.extra_caps().min(MSG_MAX_EXTRA_CAPS) as usize;
    let Some(buffer) = sender.ipc_buffer() else {
        return Ok(slots);
    };
    if requested == 0 {
        return Ok(slots);
    }

    for (i, slot_out) in slots.iter_mut().enumerate().take(requested) {
        let cptr = buffer.word(IpcBufferSlot::CapsOrBadges.index() + i);
        let Some(slot) = lookup_slot_in_tcb(sender, cptr) else {
            return Err(cptr);
        };
        *slot_out = Some(slot);
    }

    Ok(slots)
}

fn lookup_cap_in_tcb(tcb: TcbRef, cptr: u64) -> Option<(Cap, CteRef)> {
    let root = tcb.cspace_cap();
    if root.tag() != Some(CapTag::CNode) {
        return None;
    }
    let (cap, slot) = cspace::lookup_cap_in(root, cptr, cspace::WORD_BITS).ok()?;
    if cap.is_null() {
        return None;
    }
    Some((cap, slot))
}

fn lookup_slot_in_tcb(tcb: TcbRef, cptr: u64) -> Option<CteRef> {
    let root = tcb.cspace_cap();
    if root.tag() != Some(CapTag::CNode) {
        return None;
    }
    let result = cspace::lookup_slot_in(root, cptr, cspace::WORD_BITS).ok()?;
    if result.bits_remaining != 0 {
        return None;
    }
    Some(result.slot)
}

/// The empty slot the receiver nominated in its IPC buffer for an incoming
/// cap, if it named a usable one.
fn get_receive_slot(receiver: TcbRef) -> Option<CteRef> {
    let buffer = receiver.ipc_buffer()?;
    let root_cptr = buffer.word(IpcBufferSlot::ReceiveCNode.index());
    let index = buffer.word(IpcBufferSlot::ReceiveIndex.index());
    let raw_depth = buffer.word(IpcBufferSlot::ReceiveDepth.index());
    let depth = if raw_depth == 0 {
        cspace::WORD_BITS
    } else {
        raw_depth as u32
    };

    let (root_cap, _) = lookup_cap_in_tcb(receiver, root_cptr)?;
    if root_cap.tag() != Some(CapTag::CNode) || depth > cspace::WORD_BITS {
        return None;
    }
    let result = cspace::lookup_slot_in(root_cap, index, depth).ok()?;
    if result.bits_remaining != 0 || !result.slot.is_empty() {
        return None;
    }
    Some(result.slot)
}

fn transfer_fault_message(sender: TcbRef, receiver: TcbRef, badge: u64) {
    let fault = sender.fault_message();
    let length = fault.len.min(MSG_MAX_LENGTH);
    let info_out = MessageInfo::new(fault.label, 0, 0, length);

    receiver.write_fault_ipc_message_regs(badge, info_out.0, &fault.mrs, length);

    let copied_len = length.min(fault.mrs.len() as u64);
    if copied_len > MR_REG_COUNT {
        if let Some(buffer) = receiver.ipc_buffer() {
            buffer.set_words(
                1 + MR_REG_COUNT_USIZE,
                &fault.mrs[MR_REG_COUNT_USIZE..copied_len as usize],
            );
        }
    }
}

/// Look up the endpoint, badge, and permission bits for the Endpoint cap at
/// `cptr`. Returns `None` if the cap is missing or not an Endpoint.
fn lookup_endpoint(cptr: u64) -> Option<(Cap, EndpointRef, u64)> {
    let (cap, _slot) = lookup_cap_current(cptr).ok()?;
    let ep = cap.as_endpoint()?;
    Some((cap, ep, cap.endpoint_badge()))
}

/// Block `cur` on `ep` as a sender, stashing the cap badge and the "is this a
/// Call?" bit so the rendezvous logic can deliver the right semantics.
fn block_sender(
    ep: EndpointRef,
    cur: TcbRef,
    is_call: bool,
    badge: u64,
    can_grant: bool,
    can_grant_reply: bool,
    extra_cap_slots: ExtraCapSlots,
) {
    cur.dequeue();
    cur.set_blocked_sender(
        ep,
        is_call,
        badge,
        can_grant,
        can_grant_reply,
        extra_cap_slots,
    );
    ep.enqueue_waiter(cur, EpState::Sending);
}

fn block_receiver(ep: EndpointRef, cur: TcbRef, can_grant: bool) {
    cur.dequeue();
    cur.set_blocked_receiver(ep, can_grant);
    ep.enqueue_waiter(cur, EpState::Receiving);
}

fn park_call_sender(sender: TcbRef, receiver: TcbRef, can_grant: bool) {
    if crate::object::reply::setup_caller_cap(sender, receiver, can_grant) {
        sender.finish_call_sender_after_rendezvous(true);
    } else if sender.sender_fault().0 {
        sender.set_inactive();
        sender.clear_waiting_on();
    } else {
        sender.deactivate_queued_call_sender();
    }
}

fn consume_bound_notification_if_active(cur: TcbRef, uc: &mut UserContext) -> bool {
    let Some(badge) = cur.bound_notification().and_then(|n| n.consume_active()) else {
        return false;
    };
    write_bound_notification_reply(cur, uc, badge);
    true
}

fn write_bound_notification_reply(cur: TcbRef, uc: &mut UserContext, badge: u64) {
    uc.set_cap_reg(badge);
    uc.set_msg_info(0);
    for i in 0..MR_REG_COUNT_USIZE {
        uc.set_mr(i, 0);
    }
    if let Some(buffer) = cur.ipc_buffer() {
        buffer.zero_words(1, MR_REG_COUNT_USIZE);
    }
}

fn complete_receive_from_sender(
    cur: TcbRef,
    sender: TcbRef,
    ep: EndpointRef,
    receiver_can_grant: bool,
) {
    let sender_state = sender.queued_sender();
    let info_in = MessageInfo(sender_state.info_word);
    if sender_state.is_fault {
        transfer_fault_message(sender, cur, sender_state.badge);
    } else {
        transfer_message(
            sender,
            cur,
            info_in,
            sender_state.badge,
            Some(ep),
            sender_state.can_grant,
            sender_state.extra_cap_slots,
        );
    }
    if sender_state.is_call {
        if sender_state.can_grant || sender_state.can_grant_reply {
            park_call_sender(sender, cur, receiver_can_grant);
        } else {
            sender.deactivate_queued_call_sender();
        }
    } else {
        sender.wake_queued_sender();
        sender.enqueue();
    }
}

/// `seL4_Send` on an Endpoint. `blocking` controls whether we queue
/// (true → `seL4_Send`) or drop (false → `seL4_NBSend`) when no
/// receiver is waiting.
pub fn send(uc: &mut UserContext, blocking: bool, _reply_rights: bool) {
    let cptr = uc.cap_reg();
    let info = MessageInfo(uc.msg_info());

    let Some((cap, ep, badge)) = lookup_endpoint(cptr) else {
        return;
    };
    if !cap.endpoint_can_send() {
        return;
    }

    let Some(cur) = tcb::current() else {
        return;
    };
    let extra_cap_slots = match snapshot_extra_cap_slots(cur, info, cap.endpoint_can_grant()) {
        Ok(slots) => slots,
        Err(bad_cptr) => {
            if blocking {
                let _ = crate::arch::current::api::send_cap_fault_ipc(uc, bad_cptr, false);
            }
            return;
        }
    };
    let Some(receiver) = ep.pop_receiver() else {
        if blocking {
            block_sender(
                ep,
                cur,
                false,
                badge,
                cap.endpoint_can_grant(),
                cap.endpoint_can_grant_reply(),
                extra_cap_slots,
            );
        }
        return;
    };
    transfer_message(
        cur,
        receiver,
        info,
        badge,
        Some(ep),
        cap.endpoint_can_grant(),
        extra_cap_slots,
    );
    receiver.wake_blocked_receiver_after_send();
    receiver.enqueue();
}

/// `seL4_Recv` on an Endpoint. Returns a synthesised reply (badge=0,
/// label=0, length=0) if no sender is waiting and `blocking=false`.
pub fn recv(uc: &mut UserContext, blocking: bool) {
    let cptr = uc.cap_reg();

    let Some((cap, ep, _)) = lookup_endpoint(cptr) else {
        write_empty_reply(uc);
        return;
    };
    if !cap.endpoint_can_receive() {
        if !crate::arch::current::api::send_cap_fault_ipc(uc, cptr, true) {
            write_empty_reply(uc);
        }
        return;
    }

    let Some(cur) = tcb::current() else {
        write_empty_reply(uc);
        return;
    };
    crate::object::reply::delete_caller_cap(cur);
    if let Some(sender) = ep.pop_sender() {
        complete_receive_from_sender(cur, sender, ep, cap.endpoint_can_grant());
        return;
    }

    if !blocking {
        // Before returning an empty non-blocking receive, check the bound
        // Notification. The C kernel's `receiveIPC` path does the same when
        // the TCB has a bound ntfn that's Active.
        if !consume_bound_notification_if_active(cur, uc) {
            write_empty_reply(uc);
        }
        return;
    }

    // A blocking receive on a thread with a bound notification also has to
    // consider a latched signal before parking on the endpoint.
    if let Some(badge) = cur.bound_notification().and_then(|n| n.consume_active()) {
        write_bound_notification_reply(cur, uc, badge);
        return;
    }
    block_receiver(ep, cur, cap.endpoint_can_grant());
}

/// `seL4_Call`. Equivalent to a blocking Send followed by an explicit wait for
/// the matching Reply. Rendezvous transfers the message, binds the receiver's
/// reply object to the caller, and parks the caller on `BlockedOnReply`. No
/// receiver waiting -> queue as a Call sender.
pub fn call(uc: &mut UserContext) {
    let cptr = uc.cap_reg();
    let info = MessageInfo(uc.msg_info());

    let Some((cap, ep, badge)) = lookup_endpoint(cptr) else {
        return; // syscall.rs falls back to its existing handler
    };
    if !cap.endpoint_can_send() {
        return;
    }

    let Some(cur) = tcb::current() else {
        return;
    };
    let extra_cap_slots = match snapshot_extra_cap_slots(cur, info, cap.endpoint_can_grant()) {
        Ok(slots) => slots,
        Err(bad_cptr) => {
            let _ = crate::arch::current::api::send_cap_fault_ipc(uc, bad_cptr, false);
            return;
        }
    };
    let Some(receiver) = ep.pop_receiver() else {
        block_sender(
            ep,
            cur,
            true,
            badge,
            cap.endpoint_can_grant(),
            cap.endpoint_can_grant_reply(),
            extra_cap_slots,
        );
        return;
    };
    transfer_message(
        cur,
        receiver,
        info,
        badge,
        Some(ep),
        cap.endpoint_can_grant(),
        extra_cap_slots,
    );
    let receiver_can_grant = receiver.start_receiver_rendezvous();
    cur.dequeue();
    if cap.endpoint_can_grant() || cap.endpoint_can_grant_reply() {
        park_call_sender(cur, receiver, receiver_can_grant);
    } else {
        cur.finish_call_sender_after_rendezvous(false);
    }
    receiver.finish_receiver_rendezvous();
    receiver.enqueue();
}

/// `seL4_Reply`: transfer to the TCB named by the current `tcbCaller` cap.
pub fn reply(uc: &mut UserContext) {
    let Some(cur) = tcb::current() else {
        return;
    };
    if let Some(slot) = crate::object::reply::caller_reply_cap(cur) {
        handle_reply_slot(uc, slot);
    }
}

pub fn handle_reply_slot(uc: &mut UserContext, (cap, slot): (Cap, CteRef)) {
    let Some(caller) = cap.as_reply_thread() else {
        return;
    };
    if cap.reply_is_master() || Some(caller) == tcb::current() {
        return;
    }
    reply_to_tcb(uc, caller, cap.reply_can_grant());
    crate::api::invocation::cte_delete_one(slot);
}

pub fn reply_to_tcb(uc: &mut UserContext, caller: TcbRef, can_grant: bool) {
    let Some(cur) = tcb::current() else {
        return;
    };
    let info = MessageInfo(uc.msg_info());
    let mut wake_caller = true;
    let (was_fault, fault_label) = caller.sender_fault();
    if !was_fault {
        transfer_message(cur, caller, info, 0, None, can_grant, NO_EXTRA_CAPS);
    } else if info.label() == 0 {
        match fault_label {
            label if label == FaultLabel::UnknownSyscall.raw() => {
                apply_unknown_syscall_reply(cur, uc, caller, info.length())
            }
            label if label == FaultLabel::UserException.raw() => {
                apply_user_exception_reply(cur, uc, caller, info.length())
            }
            _ => {}
        }
    } else {
        // A non-zero label means the handler refused to resume the fault, and
        // these two fault kinds have no other way to make progress.
        let no_resume = fault_label == FaultLabel::UnknownSyscall.raw()
            || fault_label == FaultLabel::UserException.raw();
        if no_resume {
            wake_caller = false;
        }
    }
    caller.finish_reply_state(was_fault, wake_caller);
    if wake_caller {
        crate::ktypes::list::clear_links(caller);
        caller.enqueue();
    }
}

fn apply_user_exception_reply(sender: TcbRef, uc: &UserContext, caller: TcbRef, length: u64) {
    let n = (length as usize).min(2);
    let mut pc = None;
    let mut regs = [(0usize, 0u64); 1];
    let mut reg_count = 0;
    if n >= 1 {
        pc = Some(reply_mr(sender, uc, 0));
    }
    if n >= 2 {
        regs[reg_count] = (USER_EXCEPTION_SP_REG, reply_mr(sender, uc, 1));
        reg_count += 1;
    }
    caller.write_user_context(pc, &regs[..reg_count]);
}

/// Message register `i` of the replying thread, from its registers for the
/// first few and from its IPC buffer beyond that.
fn reply_mr(sender: TcbRef, uc: &UserContext, i: usize) -> u64 {
    match i {
        0..MR_REG_COUNT_USIZE => uc.mr(i),
        _ => sender
            .ipc_buffer()
            .map_or(0, |buffer: IpcBuffer| buffer.word(1 + i)),
    }
}

fn apply_unknown_syscall_reply(sender: TcbRef, uc: &UserContext, caller: TcbRef, length: u64) {
    let n = (length as usize).min(UNKNOWN_SYSCALL_REPLY_REGS.len());
    let mut pc = None;
    let mut regs = [(0usize, 0u64); 24];
    let mut reg_count = 0;
    for (i, reg) in UNKNOWN_SYSCALL_REPLY_REGS
        .iter()
        .copied()
        .enumerate()
        .take(n)
    {
        let value = reply_mr(sender, uc, i);
        if i == UNKNOWN_SYSCALL_FAULT_IP_MR {
            pc = Some(value);
        } else if reg != 0 {
            regs[reg_count] = (reg, value);
            reg_count += 1;
        }
    }
    caller.write_user_context(pc, &regs[..reg_count]);
}

/// `seL4_ReplyRecv`: Reply from `tcbCaller`, then Recv on the EP cap.
pub fn reply_recv(uc: &mut UserContext) {
    reply(uc);
    recv(uc, true);
}

/// "No sender, no payload" reply written into the syscall return
/// registers. Used by `recv` when there's nothing pending and the
/// caller asked for non-blocking semantics (or the cap was bogus).
/// Clears the returned badge/info/MR registers so userspace never observes
/// stale trap-entry state for an empty receive.
fn write_empty_reply(uc: &mut UserContext) {
    uc.set_cap_reg(0);
    uc.set_msg_info(0);
    for i in 0..MR_REG_COUNT_USIZE {
        uc.set_mr(i, 0);
    }
    // Clear MR[0..3] in the IPC buffer too so seL4_GetMR sees zeros.
    if let Some(buffer) = tcb::current().and_then(TcbRef::ipc_buffer) {
        buffer.zero_words(1, MR_REG_COUNT_USIZE);
    }
}
