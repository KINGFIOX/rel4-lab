//! Reply caps: a master cap in `tcbReply` and a derived caller cap.
//!
//! A reply cap points at a TCB. The master lives in the sender's `tcbReply`
//! slot; `setup_caller_cap` inserts a derived cap into the receiver's
//! `tcbCaller` slot.

#![allow(dead_code)]
// This module is written entirely in terms of the safe abstractions in
// `ktypes`; keep it that way.
#![deny(unsafe_code)]

use crate::object::cap::{Cap, CapTag};
use crate::object::cnode::CteRef;
use crate::object::mdb::MdbNode;
use crate::object::tcb::{self, TcbRef};

pub fn setup_reply_master(tcb: TcbRef) {
    let Some(slot) = tcb.cap_slot(tcb::TCB_REPLY) else {
        return;
    };
    if !slot.cap().is_null() {
        return;
    }
    slot.with_mut(|cte| {
        cte.cap = Cap::new_reply(tcb.kva(), true, true);
        cte.mdb = MdbNode::new(0, 0, true, true);
    });
}

pub fn setup_caller_cap(sender: TcbRef, receiver: TcbRef, can_grant: bool) -> bool {
    setup_reply_master(sender);
    sender.set_blocked_on_reply();
    let (Some(reply_slot), Some(caller_slot)) = (
        sender.cap_slot(tcb::TCB_REPLY),
        receiver.cap_slot(tcb::TCB_CALLER),
    ) else {
        return false;
    };
    let master = reply_slot.cap();
    if master.tag() != Some(CapTag::Reply)
        || !master.reply_is_master()
        || master.reply_tcb_ptr() != sender.kva()
    {
        return false;
    }
    if !caller_slot.cap().is_null() {
        delete_caller_cap(receiver);
    }
    let derived = Cap::new_reply(sender.kva(), can_grant, false);
    reply_slot.cte_insert(derived, caller_slot);
    true
}

pub fn delete_caller_cap(receiver: TcbRef) {
    if let Some(caller_slot) = receiver.cap_slot(tcb::TCB_CALLER) {
        crate::api::invocation::cte_delete_one(caller_slot);
    }
}

pub fn cancel_blocked_on_reply(tcb: TcbRef) {
    tcb.clear_fault_message();
    let Some(reply_slot) = tcb.cap_slot(tcb::TCB_REPLY) else {
        return;
    };
    if let Some(derived) = reply_slot.mdb_next() {
        crate::api::invocation::cte_delete_one(derived);
    }
}

/// The caller cap a receiver holds, if any, together with its slot.
pub fn caller_reply_cap(receiver: TcbRef) -> Option<(Cap, CteRef)> {
    let slot = receiver.cap_slot(tcb::TCB_CALLER)?;
    Some((slot.cap(), slot))
}
