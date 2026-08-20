//! Non-MCS reply caps: a master cap in `tcbReply` and a derived caller cap.
//!
//! Upstream seL4 without `CONFIG_KERNEL_MCS` does not retype a Reply object.
//! `cap_reply_cap` points at a TCB. The master lives in the sender's
//! `tcbReply` slot; `setupCallerCap` inserts a derived cap into the
//! receiver's `tcbCaller` slot.

#![allow(dead_code)]

use crate::object::cap::{Cap, CapTag};
use crate::object::cnode::{self, Cte};
use crate::object::mdb::MdbNode;
use crate::object::tcb::{self, Tcb};

pub unsafe fn setup_reply_master(tcb: *mut Tcb) {
    if tcb.is_null() {
        return;
    }
    unsafe {
        let slot = tcb::cap_slot(tcb, tcb::TCB_REPLY);
        if slot.is_null() {
            return;
        }
        let _guard = cnode::lock_cspace();
        if !(*slot).cap.is_null() {
            return;
        }
        (*slot).cap = Cap::new_reply(tcb as u64, true, true);
        (*slot).mdb = MdbNode::new(0, 0, true, true);
    }
}

pub unsafe fn setup_caller_cap(sender: *mut Tcb, receiver: *mut Tcb, can_grant: bool) -> bool {
    if sender.is_null() || receiver.is_null() {
        return false;
    }
    unsafe {
        setup_reply_master(sender);
        tcb::set_blocked_on_reply(sender);
        let reply_slot = tcb::cap_slot(sender, tcb::TCB_REPLY);
        let caller_slot = tcb::cap_slot(receiver, tcb::TCB_CALLER);
        if reply_slot.is_null() || caller_slot.is_null() {
            return false;
        }
        let master = cnode::cap_snapshot(reply_slot);
        if master.tag() != Some(CapTag::Reply)
            || !master.reply_is_master()
            || master.reply_tcb_ptr() != sender as u64
        {
            return false;
        }
        if !(*caller_slot).cap.is_null() {
            delete_caller_cap(receiver);
        }
        let derived = Cap::new_reply(sender as u64, can_grant, false);
        let cspace_guard = cnode::lock_cspace();
        cnode::cte_insert_locked(&cspace_guard, derived, reply_slot, caller_slot);
    }
    true
}

pub unsafe fn delete_caller_cap(receiver: *mut Tcb) {
    if receiver.is_null() {
        return;
    }
    unsafe {
        let caller_slot = tcb::cap_slot(receiver, tcb::TCB_CALLER);
        if caller_slot.is_null() {
            return;
        }
        crate::api::invocation::cte_delete_one(caller_slot);
    }
}

pub unsafe fn cancel_blocked_on_reply(tcb: *mut Tcb) {
    if tcb.is_null() {
        return;
    }
    unsafe {
        tcb::clear_fault_message(tcb);
        let reply_slot = tcb::cap_slot(tcb, tcb::TCB_REPLY);
        if reply_slot.is_null() {
            return;
        }
        let next = {
            let _guard = cnode::lock_cspace();
            (*reply_slot).mdb.next()
        };
        if next != 0 {
            crate::api::invocation::cte_delete_one(next as *mut Cte);
        }
    }
}

pub unsafe fn caller_reply_cap(receiver: *mut Tcb) -> (Cap, *mut Cte) {
    if receiver.is_null() {
        return (Cap::null(), core::ptr::null_mut());
    }
    unsafe {
        let slot = tcb::cap_slot(receiver, tcb::TCB_CALLER);
        if slot.is_null() {
            return (Cap::null(), core::ptr::null_mut());
        }
        (cnode::cap_snapshot(slot), slot)
    }
}
