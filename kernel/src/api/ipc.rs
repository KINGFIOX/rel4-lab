//! Endpoint-IPC core: message transfer plus the Send / Recv / Call /
//! Reply / ReplyRecv state-machine glue that bridges `syscall::do_*`
//! to `object::endpoint`.
//!
//! Design follows the explicit reply-cap IPC path:
//!
//! * `seL4_Send` (blocking) and `seL4_Call` walk the EP. If a receiver
//!   is already waiting → rendezvous, transfer message, wake the
//!   receiver. `Call` additionally parks the sender on `BlockedOnReply`
//!   only when the receiver supplied a valid reply object. If no receiver is
//!   waiting, the sender is dequeued from the runqueue and queued on the EP
//!   (`BlockedOnSend`).
//! * `seL4_NBSend` is the Send path minus the queueing fallback —
//!   no receiver waiting means the message is dropped.
//! * `seL4_Recv` (blocking) walks the EP for a queued sender, and if
//!   one is found rendezvous + wake-or-park-the-sender (depending on
//!   whether the queued sender was a Call). No sender → block on the
//!   EP (`BlockedOnReceive`).
//! * `seL4_NBRecv` is Recv minus the blocking fallback.
//! * Reply delivery comes through an explicit reply object.
//! * `seL4_ReplyRecv` is Reply followed by Recv on a fresh cap/reply object.
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

use crate::abi::fault::FaultLabel;
use crate::abi::types::MessageInfo;
use crate::api::cspace::{self, lookup_cap};
use crate::api::invocation::derive_cap_for_copy;
use crate::api::thread;
use crate::arch::current::trap::{UserContext, UserRegister};
use crate::object::cap::{Cap, CapTag};
use crate::object::cnode::Cte;
use crate::object::endpoint::{self, EpState};
use crate::object::tcb;

/// `seL4_MsgMaxLength` (libsel4/include/sel4/constants.h).
const MSG_MAX_LENGTH: u64 = 120;
const MSG_MAX_EXTRA_CAPS: u64 = 3;
const MSG_MAX_EXTRA_CAPS_USIZE: usize = MSG_MAX_EXTRA_CAPS as usize;
const MR_REG_COUNT: u64 = 4;
const MR_REG_COUNT_USIZE: usize = MR_REG_COUNT as usize;
type ExtraCapSlots = [u64; MSG_MAX_EXTRA_CAPS_USIZE];

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

/// Copy MRs from `sender` into `receiver`, set the receiver's badge +
/// reply MessageInfo. `length` is the truncated MR count to deliver.
unsafe fn transfer_message(
    sender: *mut tcb::Tcb,
    receiver: *mut tcb::Tcb,
    info_in: MessageInfo,
    badge: u64,
    endpoint: *mut endpoint::Endpoint,
    can_grant: bool,
    extra_cap_slots: ExtraCapSlots,
) {
    let label = info_in.label();
    let mut length = info_in.length();
    if length > MSG_MAX_LENGTH {
        length = MSG_MAX_LENGTH;
    }

    let mr_regs = tcb::ipc_message_regs_snapshot(sender, length);
    unsafe { tcb::write_ipc_message_regs(receiver, badge, &mr_regs, length) };

    unsafe {
        let mut transferred_length = length;
        if length > MR_REG_COUNT {
            if tcb::has_ipc_buffer(sender) && tcb::has_ipc_buffer(receiver) {
                let extra = length - MR_REG_COUNT;
                tcb::copy_ipc_buffer_words(
                    sender,
                    receiver,
                    1 + MR_REG_COUNT_USIZE,
                    extra as usize,
                );
            } else {
                transferred_length = MR_REG_COUNT;
            }
        }

        let (caps_unwrapped, extra_caps) = transfer_caps(
            sender,
            receiver,
            info_in,
            endpoint,
            can_grant,
            extra_cap_slots,
        );
        let info_out = MessageInfo::new(label, caps_unwrapped, extra_caps, transferred_length);
        tcb::write_message_info(receiver, info_out.0);
    }
}

unsafe fn transfer_caps(
    _sender: *mut tcb::Tcb,
    receiver: *mut tcb::Tcb,
    info_in: MessageInfo,
    endpoint: *mut endpoint::Endpoint,
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
    if requested == 0 {
        return (0, 0);
    }
    if extra_cap_slots[0] == 0 || !tcb::has_ipc_buffer(receiver) {
        return (0, 0);
    }

    let mut dest_slot = unsafe { get_receive_slot(receiver) };
    let mut caps_unwrapped = 0u64;
    let mut transferred = 0u64;

    for i in 0..requested as usize {
        let src_slot = extra_cap_slots[i] as *mut Cte;
        if src_slot.is_null() {
            break;
        }
        let src_cap = crate::object::cnode::cap_snapshot(src_slot);
        if src_cap.is_null() {
            break;
        }

        if src_cap.tag() == Some(CapTag::Endpoint)
            && !endpoint.is_null()
            && src_cap.endpoint_ptr() == endpoint as u64
        {
            if !unsafe {
                tcb::write_ipc_buffer_word(
                    receiver,
                    IpcBufferSlot::CapsOrBadges.index() + i,
                    src_cap.endpoint_badge(),
                )
            } {
                break;
            }
            caps_unwrapped |= 1u64 << i;
            transferred = i as u64 + 1;
            continue;
        };

        let dst = match dest_slot {
            Some(s) => s,
            None => break,
        };
        let derived = match derive_cap_for_copy(src_slot, src_cap) {
            Ok(cap) if !cap.is_null() => cap,
            _ => break,
        };
        if !unsafe { insert_derived_cap(src_slot, dst, derived) } {
            break;
        }
        dest_slot = None;

        transferred = i as u64 + 1;
    }

    (caps_unwrapped, transferred)
}

fn snapshot_extra_cap_slots(
    sender: *mut tcb::Tcb,
    info: MessageInfo,
    can_grant: bool,
) -> Result<ExtraCapSlots, u64> {
    let mut slots = [0u64; MSG_MAX_EXTRA_CAPS_USIZE];
    if sender.is_null() || !can_grant {
        return Ok(slots);
    }
    let requested = info.extra_caps().min(MSG_MAX_EXTRA_CAPS) as usize;
    if requested == 0 || !tcb::has_ipc_buffer(sender) {
        return Ok(slots);
    }

    for (i, slot_out) in slots.iter_mut().enumerate().take(requested) {
        let cptr = tcb::ipc_buffer_word_snapshot(sender, IpcBufferSlot::CapsOrBadges.index() + i);
        let Some(slot) = (unsafe { lookup_slot_in_tcb(sender, cptr) }) else {
            return Err(cptr);
        };
        *slot_out = slot as u64;
    }

    Ok(slots)
}

unsafe fn lookup_cap_in_tcb(t: *mut tcb::Tcb, cptr: u64) -> Option<(Cap, *mut Cte)> {
    if t.is_null() {
        return None;
    }
    let root = tcb::cspace_cap_snapshot(t);
    if root.tag() != Some(CapTag::CNode) {
        return None;
    }
    let (cap, slot) = cspace::lookup_cap_in(root, cptr, cspace::WORD_BITS).ok()?;
    if cap.is_null() {
        return None;
    }
    Some((cap, slot))
}

unsafe fn lookup_slot_in_tcb(t: *mut tcb::Tcb, cptr: u64) -> Option<*mut Cte> {
    if t.is_null() {
        return None;
    }
    let root = tcb::cspace_cap_snapshot(t);
    if root.tag() != Some(CapTag::CNode) {
        return None;
    }
    let r = cspace::lookup_slot_in(root, cptr, cspace::WORD_BITS).ok()?;
    if r.bits_remaining != 0 {
        return None;
    }
    Some(r.slot)
}

unsafe fn get_receive_slot(receiver: *mut tcb::Tcb) -> Option<*mut Cte> {
    let root_cptr = tcb::ipc_buffer_word_snapshot(receiver, IpcBufferSlot::ReceiveCNode.index());
    let index = tcb::ipc_buffer_word_snapshot(receiver, IpcBufferSlot::ReceiveIndex.index());
    let raw_depth = tcb::ipc_buffer_word_snapshot(receiver, IpcBufferSlot::ReceiveDepth.index());
    let depth = if raw_depth == 0 {
        cspace::WORD_BITS
    } else {
        raw_depth as u32
    };

    let (root_cap, _) = unsafe { lookup_cap_in_tcb(receiver, root_cptr) }?;
    if root_cap.tag() != Some(CapTag::CNode) || depth > cspace::WORD_BITS {
        return None;
    }
    let r = cspace::lookup_slot_in(root_cap, index, depth).ok()?;
    if r.bits_remaining != 0 {
        return None;
    }
    let empty = {
        let _cspace_guard = crate::object::cnode::lock_cspace();
        unsafe { (*r.slot).cap.is_null() && (*r.slot).mdb.prev() == 0 && (*r.slot).mdb.next() == 0 }
    };
    if !empty {
        return None;
    }
    Some(r.slot)
}

unsafe fn insert_derived_cap(src_slot: *mut Cte, dst: *mut Cte, cap: Cap) -> bool {
    unsafe {
        let cspace_guard = crate::object::cnode::lock_cspace();
        if !(*dst).cap.is_null() || (*dst).mdb.prev() != 0 || (*dst).mdb.next() != 0 {
            return false;
        }
        crate::object::cnode::cte_insert_locked(&cspace_guard, cap, src_slot, dst);
        true
    }
}

unsafe fn transfer_fault_message(sender: *mut tcb::Tcb, receiver: *mut tcb::Tcb, badge: u64) {
    let fault = tcb::fault_message_snapshot(sender);
    let mut length = fault.len;
    if length > MSG_MAX_LENGTH {
        length = MSG_MAX_LENGTH;
    }
    let info_out = MessageInfo::new(fault.label, 0, 0, length);

    unsafe { tcb::write_fault_ipc_message_regs(receiver, badge, info_out.0, &fault.mrs, length) };

    unsafe {
        let copied_len = length.min(fault.mrs.len() as u64);
        if copied_len > MR_REG_COUNT {
            tcb::write_ipc_buffer_words(
                receiver,
                1 + MR_REG_COUNT_USIZE,
                &fault.mrs[MR_REG_COUNT_USIZE..copied_len as usize],
            );
        }
    }
}

/// Look up the cap and badge / permission bits for an Endpoint
/// reference at `cptr`. Returns `None` if the cap is missing or not
/// an Endpoint.
fn lookup_endpoint(cptr: u64) -> Option<(Cap, *mut endpoint::Endpoint, u64)> {
    let (cap, _slot) = unsafe { thread::with_current(|t| lookup_cap(t, cptr)) }.ok()?;
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
    extra_cap_slots: ExtraCapSlots,
) {
    if ep.is_null() {
        return;
    }
    let cur = tcb::current();
    if cur.is_null() {
        return;
    }
    let _guard = unsafe { endpoint::lock_queue(ep) };
    unsafe {
        block_sender_locked(
            ep,
            cur,
            is_call,
            badge,
            can_grant,
            can_grant_reply,
            extra_cap_slots,
        );
    }
}

unsafe fn block_sender_locked(
    ep: *mut endpoint::Endpoint,
    cur: *mut tcb::Tcb,
    is_call: bool,
    badge: u64,
    can_grant: bool,
    can_grant_reply: bool,
    extra_cap_slots: ExtraCapSlots,
) {
    if cur.is_null() || ep.is_null() {
        return;
    }
    unsafe {
        tcb::dequeue(cur);
        tcb::set_blocked_sender(
            cur,
            ep as u64,
            is_call,
            badge,
            can_grant,
            can_grant_reply,
            extra_cap_slots,
        );
        endpoint::enqueue_waiter_locked(ep, cur, EpState::Sending);
    }
}

/// Block the current TCB on `ep` as a receiver. No payload to stash.
unsafe fn block_receiver(ep: *mut endpoint::Endpoint, can_grant: bool, reply_cptr: u64) {
    if ep.is_null() {
        return;
    }
    let cur = tcb::current();
    if cur.is_null() {
        return;
    }
    let _guard = unsafe { endpoint::lock_queue(ep) };
    unsafe {
        block_receiver_locked(ep, cur, can_grant, reply_cptr);
    }
}

unsafe fn block_receiver_locked(
    ep: *mut endpoint::Endpoint,
    cur: *mut tcb::Tcb,
    can_grant: bool,
    reply_cptr: u64,
) {
    if cur.is_null() || ep.is_null() {
        return;
    }
    unsafe {
        let (bound_reply_cptr, bound_reply_kva, bound_reply_can_grant) = if reply_cptr != 0 {
            match lookup_reply_object_for(cur, reply_cptr) {
                Some((_slot, reply_kva, reply_can_grant))
                    if crate::object::reply::prepare_receiver(reply_kva, cur) =>
                {
                    (reply_cptr, reply_kva, reply_can_grant)
                }
                _ => (0, 0, false),
            }
        } else {
            (0, 0, false)
        };
        tcb::dequeue(cur);
        tcb::set_blocked_receiver(
            cur,
            ep as u64,
            can_grant,
            bound_reply_cptr,
            bound_reply_kva,
            bound_reply_can_grant,
        );
        if bound_reply_kva != 0 {
            let bound = crate::object::reply::bind_blocked_receiver(bound_reply_kva, cur);
            debug_assert!(bound, "valid receive reply object must bind");
        }
        endpoint::enqueue_waiter_locked(ep, cur, EpState::Receiving);
    }
}
unsafe fn lookup_reply_object_for(
    owner: *mut tcb::Tcb,
    reply_cptr: u64,
) -> Option<(*mut Cte, u64, bool)> {
    if owner.is_null() || reply_cptr == 0 {
        return None;
    }
    let cspace_cap = tcb::cspace_cap_snapshot(owner);
    if cspace_cap.tag() != Some(CapTag::CNode) {
        return None;
    }
    let (reply_cap, slot) = match cspace::lookup_cap_in(cspace_cap, reply_cptr, cspace::WORD_BITS) {
        Ok(v) => v,
        _ => return None,
    };
    if reply_cap.tag() != Some(CapTag::Reply) || !reply_cap.reply_is_object() {
        return None;
    }
    Some((
        slot,
        reply_cap.reply_object_ptr(),
        reply_cap.reply_object_can_grant(),
    ))
}

unsafe fn reply_object_for_receive(
    receiver: *mut tcb::Tcb,
    reply_cptr: u64,
) -> Option<(u64, u64, bool)> {
    let (bound_reply_cptr, bound_reply_kva, bound_reply_can_grant) =
        tcb::receive_reply_snapshot(receiver);
    if bound_reply_kva != 0 {
        return Some((bound_reply_cptr, bound_reply_kva, bound_reply_can_grant));
    }
    let (_slot, reply_kva, reply_can_grant) =
        unsafe { lookup_reply_object_for(receiver, reply_cptr)? };
    unsafe {
        crate::object::reply::cancel_owner_for_receive_if_needed(reply_kva, receiver);
    }
    Some((reply_cptr, reply_kva, reply_can_grant))
}

pub(crate) unsafe fn set_reply_object_for(
    receiver: *mut tcb::Tcb,
    reply_cptr: u64,
    reply_kva: u64,
    reply_can_grant: bool,
    caller: *mut tcb::Tcb,
    can_grant: bool,
    _reply_rights: bool,
) -> bool {
    if receiver.is_null() || caller.is_null() || reply_kva == 0 {
        return false;
    }
    unsafe {
        if !matches!(
            lookup_reply_object_for(receiver, reply_cptr),
            Some((_slot, looked_up_reply_kva, _)) if looked_up_reply_kva == reply_kva
        ) {
            return false;
        }
        if !crate::object::reply::push(
            caller,
            receiver,
            reply_kva,
            false,
            can_grant && reply_can_grant,
        ) {
            return false;
        }
        true
    }
}
unsafe fn consume_bound_notification_if_active(cur: *mut tcb::Tcb, uc: &mut UserContext) -> bool {
    if cur.is_null() {
        return false;
    }
    let bound = tcb::bound_notification_snapshot(cur);
    if bound == 0 {
        return false;
    }
    let ntfn = bound as *mut crate::object::notification::Notification;
    let Some(badge) = (unsafe { crate::object::notification::consume_active(ntfn, cur) }) else {
        return false;
    };

    unsafe {
        write_bound_notification_reply(cur, uc, badge);
    }
    true
}

unsafe fn write_bound_notification_reply(cur: *mut tcb::Tcb, uc: &mut UserContext, badge: u64) {
    unsafe {
        uc.regs[UserRegister::A0.index()] = badge;
        uc.regs[UserRegister::A1.index()] = 0;
        uc.regs[UserRegister::A2.index()] = 0;
        uc.regs[UserRegister::A3.index()] = 0;
        uc.regs[UserRegister::A4.index()] = 0;
        uc.regs[UserRegister::A5.index()] = 0;
        tcb::zero_ipc_buffer_words(cur, 1, MR_REG_COUNT as usize);
    }
}

unsafe fn complete_receive_from_sender(
    cur: *mut tcb::Tcb,
    sender: *mut tcb::Tcb,
    ep: *mut endpoint::Endpoint,
    reply_cptr: u64,
) {
    if cur.is_null() || sender.is_null() {
        return;
    }
    let sender_state = tcb::queued_sender_snapshot(sender);
    let info_in = MessageInfo(sender_state.info_word);
    unsafe {
        let receive_reply =
            if sender_state.is_call && (sender_state.can_grant || sender_state.can_grant_reply) {
                reply_object_for_receive(cur, reply_cptr)
            } else {
                None
            };
        if sender_state.is_fault {
            transfer_fault_message(sender, cur, sender_state.badge);
        } else {
            transfer_message(
                sender,
                cur,
                info_in,
                sender_state.badge,
                ep,
                sender_state.can_grant,
                sender_state.extra_cap_slots,
            );
        }
        if sender_state.is_call {
            if sender_state.can_grant || sender_state.can_grant_reply {
                let (reply_set, reply_token, reply_can_grant) = match receive_reply {
                    Some((reply_cptr, reply_kva, reply_can_grant)) => (
                        set_reply_object_for(
                            cur,
                            reply_cptr,
                            reply_kva,
                            reply_can_grant,
                            sender,
                            sender_state.can_grant,
                            false,
                        ),
                        reply_kva,
                        sender_state.can_grant && reply_can_grant,
                    ),
                    None => {
                        tcb::set_blocked_on_reply(sender, (sender as u64) | 1);
                        (true, (sender as u64) | 1, false)
                    }
                };
                if reply_set {
                    tcb::set_caller_reply(cur, reply_token, reply_can_grant);
                    // Caller is now parked on the reply object.
                } else if sender_state.is_fault {
                    tcb::set_inactive(sender);
                    tcb::clear_waiting_on(sender);
                } else {
                    tcb::deactivate_queued_call_sender(sender);
                };
            } else {
                tcb::deactivate_queued_call_sender(sender);
            }
        } else {
            // Plain Send: wake the sender, drop its badge.
            tcb::wake_queued_sender(sender);
            tcb::enqueue(sender);
        }
    }
}

/// `seL4_Send` on an Endpoint. `blocking` controls whether we queue
/// (true → `seL4_Send`) or drop (false → `seL4_NBSend`) when no
/// receiver is waiting.
pub fn send(uc: &mut UserContext, blocking: bool, _reply_rights: bool) {
    let cptr = uc.regs[UserRegister::A0.index()];
    let info = MessageInfo(uc.regs[UserRegister::A1.index()]);

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
    let extra_cap_slots = match snapshot_extra_cap_slots(cur, info, cap.endpoint_can_grant()) {
        Ok(slots) => slots,
        Err(bad_cptr) => {
            if blocking {
                let _ = crate::arch::current::trap::send_cap_fault_ipc(uc, bad_cptr, false);
            }
            return;
        }
    };
    let receiver = unsafe {
        let _guard = endpoint::lock_queue(ep);
        let receiver = endpoint::pop_receiver_locked(ep);
        if receiver.is_null() && blocking {
            block_sender_locked(
                ep,
                cur,
                false,
                badge,
                cap.endpoint_can_grant(),
                cap.endpoint_can_grant_reply(),
                extra_cap_slots,
            );
        }
        receiver
    };
    if receiver.is_null() {
        return;
    }
    unsafe {
        transfer_message(
            cur,
            receiver,
            info,
            badge,
            ep,
            cap.endpoint_can_grant(),
            extra_cap_slots,
        );
        tcb::wake_blocked_receiver_after_send(receiver);
        tcb::enqueue(receiver);
    }
}

/// `seL4_Recv` on an Endpoint. Returns synthesised reply (badge=0,
/// label=0, length=0) if no sender is waiting and `blocking=false`.
pub fn recv(uc: &mut UserContext, blocking: bool) {
    recv_with_reply(uc, blocking, 0)
}
pub fn recv_mcs(uc: &mut UserContext, blocking: bool, reply_cptr: u64) {
    recv_with_reply(uc, blocking, reply_cptr)
}

fn recv_with_reply(uc: &mut UserContext, blocking: bool, reply_cptr: u64) {
    let cptr = uc.regs[UserRegister::A0.index()];

    let (cap, ep, _) = match lookup_endpoint(cptr) {
        Some(v) => v,
        None => {
            write_empty_reply(uc);
            return;
        }
    };
    if !cap.endpoint_can_receive() {
        if !crate::arch::current::trap::send_cap_fault_ipc(uc, cptr, true) {
            write_empty_reply(uc);
        }
        return;
    }

    let cur = tcb::current();
    if cur.is_null() {
        write_empty_reply(uc);
        return;
    }
    let sender = unsafe {
        let _guard = endpoint::lock_queue(ep);
        endpoint::pop_sender_locked(ep)
    };
    if !sender.is_null() {
        unsafe { complete_receive_from_sender(cur, sender, ep, reply_cptr) };
        return;
    }

    if !blocking {
        // Before returning an empty non-blocking receive, check the bound
        // Notification. The C kernel's `receiveIPC` path does the same when
        // the TCB has a bound ntfn that's Active.
        if unsafe { consume_bound_notification_if_active(cur, uc) } {
            return;
        }
        write_empty_reply(uc);
        return;
    }

    enum RecvBlockAction {
        Sender(*mut tcb::Tcb),
        Notification(u64),
        Blocked,
    }

    let bound = tcb::bound_notification_snapshot(cur);
    if bound != 0 {
        let ntfn = bound as *mut crate::object::notification::Notification;
        let action = unsafe {
            let _guard = crate::object::wait_queue_lock::lock_pair(ntfn.cast(), ep.cast());
            let sender = endpoint::pop_sender_locked(ep);
            if !sender.is_null() {
                RecvBlockAction::Sender(sender)
            } else if let Some(badge) =
                crate::object::notification::consume_active_locked(ntfn, cur)
            {
                RecvBlockAction::Notification(badge)
            } else {
                block_receiver_locked(ep, cur, cap.endpoint_can_grant(), reply_cptr);
                RecvBlockAction::Blocked
            }
        };
        match action {
            RecvBlockAction::Sender(sender) => {
                unsafe { complete_receive_from_sender(cur, sender, ep, reply_cptr) };
            }
            RecvBlockAction::Notification(badge) => unsafe {
                write_bound_notification_reply(cur, uc, badge);
            },
            RecvBlockAction::Blocked => {}
        }
    } else {
        let sender = unsafe {
            let _guard = endpoint::lock_queue(ep);
            let sender = endpoint::pop_sender_locked(ep);
            if sender.is_null() {
                block_receiver_locked(ep, cur, cap.endpoint_can_grant(), reply_cptr);
            }
            sender
        };
        if !sender.is_null() {
            unsafe { complete_receive_from_sender(cur, sender, ep, reply_cptr) };
        }
    }
}

/// `seL4_Call`. Equivalent to a blocking Send followed by an explicit wait for
/// the matching Reply. Rendezvous transfers the message, binds the receiver's
/// reply object to the caller, and parks the caller on `BlockedOnReply`. No
/// receiver waiting -> queue as a Call sender.
pub fn call(uc: &mut UserContext) {
    let cptr = uc.regs[UserRegister::A0.index()];
    let info = MessageInfo(uc.regs[UserRegister::A1.index()]);

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
    let extra_cap_slots = match snapshot_extra_cap_slots(cur, info, cap.endpoint_can_grant()) {
        Ok(slots) => slots,
        Err(bad_cptr) => {
            let _ = crate::arch::current::trap::send_cap_fault_ipc(uc, bad_cptr, false);
            return;
        }
    };
    let receiver = unsafe {
        let _guard = endpoint::lock_queue(ep);
        let receiver = endpoint::pop_receiver_locked(ep);
        if receiver.is_null() {
            block_sender_locked(
                ep,
                cur,
                true,
                badge,
                cap.endpoint_can_grant(),
                cap.endpoint_can_grant_reply(),
                extra_cap_slots,
            );
        }
        receiver
    };
    if receiver.is_null() {
        return;
    }
    unsafe {
        transfer_message(
            cur,
            receiver,
            info,
            badge,
            ep,
            cap.endpoint_can_grant(),
            extra_cap_slots,
        );
        let (receiver_reply_cptr, receiver_reply_kva, receiver_reply_can_grant) =
            tcb::start_receiver_rendezvous(receiver);
        tcb::dequeue(cur);
        let has_reply_rights = cap.endpoint_can_grant() || cap.endpoint_can_grant_reply();
        let reply_token = if has_reply_rights {
            if receiver_reply_kva != 0 {
                let reply_set = set_reply_object_for(
                    receiver,
                    receiver_reply_cptr,
                    receiver_reply_kva,
                    receiver_reply_can_grant,
                    cur,
                    cap.endpoint_can_grant(),
                    true,
                );
                if reply_set {
                    receiver_reply_kva
                } else {
                    tcb::set_blocked_on_reply(cur, (cur as u64) | 1);
                    (cur as u64) | 1
                }
            } else {
                tcb::set_blocked_on_reply(cur, (cur as u64) | 1);
                (cur as u64) | 1
            }
        } else {
            0
        };
        if reply_token != 0 {
            tcb::set_caller_reply(
                receiver,
                reply_token,
                cap.endpoint_can_grant() && receiver_reply_can_grant && receiver_reply_kva != 0,
            );
        }
        tcb::finish_call_sender_after_rendezvous(cur, reply_token != 0);
        tcb::finish_receiver_rendezvous(receiver);
        tcb::enqueue(receiver);
    }
}

/// Reply delivery is driven by Send on an explicit Reply cap, so this
/// compatibility hook is a no-op.
pub fn reply(uc: &mut UserContext) {
    let cur = tcb::current();
    if cur.is_null() {
        return;
    }
    let (reply_kva, _can_grant) = unsafe { tcb::take_caller_reply(cur) };
    if reply_kva == 0 {
        return;
    }
    let caller = if reply_kva & 1 != 0 {
        (reply_kva & !1) as *mut tcb::Tcb
    } else {
        unsafe { crate::object::reply::tcb(reply_kva) }
    };
    unsafe {
        reply_to_tcb(uc, caller);
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
    let info = MessageInfo(uc.regs[UserRegister::A1.index()]);
    unsafe {
        let mut wake_caller = true;
        let (was_fault, fault_label) = tcb::sender_fault_snapshot(caller);
        if !was_fault {
            transfer_message(
                cur,
                caller,
                info,
                0,
                core::ptr::null_mut(),
                false,
                [0; MSG_MAX_EXTRA_CAPS_USIZE],
            );
        } else {
            if info.label() == 0 {
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
                let no_resume = fault_label == FaultLabel::UnknownSyscall.raw()
                    || fault_label == FaultLabel::UserException.raw();
                if no_resume {
                    wake_caller = false;
                }
            }
        }
        tcb::finish_reply_state(caller, was_fault, wake_caller);
        if wake_caller {
            tcb::clear_queue_links(caller);
            tcb::enqueue(caller);
        }
    }
}

unsafe fn apply_user_exception_reply(
    sender: *mut tcb::Tcb,
    uc: &UserContext,
    caller: *mut tcb::Tcb,
    length: u64,
) {
    let n = (length as usize).min(2);
    let mut pc = None;
    let mut regs = [(0usize, 0u64); 1];
    let mut reg_count = 0;
    unsafe {
        if n >= 1 {
            pc = Some(reply_mr(sender, uc, 0));
        }
        if n >= 2 {
            regs[reg_count] = (UserRegister::Sp.index(), reply_mr(sender, uc, 1));
            reg_count += 1;
        }
        tcb::write_user_context(caller, pc, &regs[..reg_count]);
    }
}

unsafe fn reply_mr(sender: *mut tcb::Tcb, uc: &UserContext, i: usize) -> u64 {
    match i {
        0 => uc.regs[UserRegister::A2.index()],
        1 => uc.regs[UserRegister::A3.index()],
        2 => uc.regs[UserRegister::A4.index()],
        3 => uc.regs[UserRegister::A5.index()],
        _ => unsafe {
            let buf = tcb::ipc_buffer_kva_snapshot(sender);
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
    length: u64,
) {
    const SYSCALL_REPLY_REGS: [usize; 10] = [
        0,
        UserRegister::Sp.index(),
        UserRegister::Ra.index(),
        UserRegister::A0.index(),
        UserRegister::A1.index(),
        UserRegister::A2.index(),
        UserRegister::A3.index(),
        UserRegister::A4.index(),
        UserRegister::A5.index(),
        UserRegister::A6.index(),
    ];

    let n = (length as usize).min(SYSCALL_REPLY_REGS.len());
    let mut pc = None;
    let mut regs = [(0usize, 0u64); SYSCALL_REPLY_REGS.len() - 1];
    let mut reg_count = 0;
    unsafe {
        for (i, reg) in SYSCALL_REPLY_REGS.iter().copied().enumerate().take(n) {
            let value = reply_mr(sender, uc, i);
            if i == 0 {
                pc = Some(value);
            } else if reg != 0 {
                regs[reg_count] = (reg, value);
                reg_count += 1;
            }
        }
        tcb::write_user_context(caller, pc, &regs[..reg_count]);
    }
}

/// `seL4_ReplyRecv`: send on the explicit Reply cap selected by the syscall
/// wrapper, then immediately Recv on the supplied EP cap.
pub fn reply_recv(uc: &mut UserContext) {
    let reply_cptr = uc.regs[UserRegister::A6.index()];
    if reply_cptr != 0 {
        crate::api::syscall::do_reply_recv_mcs(uc);
    } else {
        reply(uc);
        recv(uc, true);
    }
}

/// "No sender, no payload" reply written into the syscall return
/// registers. Used by `recv` when there's nothing pending and the
/// caller asked for non-blocking semantics (or the cap was bogus).
/// Clears the returned badge/info/MR registers so userspace never observes
/// stale trap-entry state for an empty receive.
fn write_empty_reply(uc: &mut UserContext) {
    uc.regs[UserRegister::A0.index()] = 0;
    uc.regs[UserRegister::A1.index()] = 0;
    uc.regs[UserRegister::A2.index()] = 0;
    uc.regs[UserRegister::A3.index()] = 0;
    uc.regs[UserRegister::A4.index()] = 0;
    uc.regs[UserRegister::A5.index()] = 0;
    // Clear MR[0..3] in the IPC buffer too so seL4_GetMR sees zeros.
    thread::zero_current_ipc_buffer_words(1, MR_REG_COUNT as usize);
}
