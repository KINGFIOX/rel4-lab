//! Minimal explicit reply object.

#![allow(dead_code)]

use crate::kernel::smp::BklObjectGuard;
use crate::object::cap::CapTag;
use crate::object::cnode::{self, Cte};
use crate::object::tcb::{self, Tcb};

pub(crate) type ReplyLockGuard = BklObjectGuard;

#[repr(C)]
pub struct Reply {
    pub tcb: u64,
    pub prev: u64,
    pub next: u64,
    pub can_grant: u64,
}

const _: () = {
    assert!(core::mem::size_of::<Reply>() == 32);
    assert!(core::mem::align_of::<Reply>() >= 8);
};

const CALL_STACK_HEAD: u64 = 1 << 48;
const CALL_STACK_PTR_MASK: u64 = 0x7f_ffff_ffff;
const CALL_STACK_SIGN_BIT: u64 = 1 << 38;
const CALL_STACK_SIGN_EXT: u64 = 0xffff_ff80_0000_0000;

#[inline]
pub(crate) fn lock(_reply_kva: u64) -> ReplyLockGuard {
    BklObjectGuard::new()
}

#[inline]
fn lock_call_stack() -> BklObjectGuard {
    BklObjectGuard::new()
}

#[inline]
fn is_kernel_pspace_kva(kva: u64) -> bool {
    kva >= crate::abi::constants::PPTR_BASE as u64 && kva < crate::abi::constants::PPTR_TOP as u64
}

#[inline]
unsafe fn clear_locked(reply: *mut Reply) {
    unsafe {
        (*reply).tcb = 0;
        (*reply).prev = 0;
        (*reply).next = 0;
        (*reply).can_grant = 0;
    }
}

unsafe fn clear(reply_kva: u64) {
    let _guard = lock(reply_kva);
    unsafe {
        clear_locked(reply_kva as *mut Reply);
    }
}

#[inline]
fn call_stack_new(ptr: u64, is_head: bool) -> u64 {
    (ptr & CALL_STACK_PTR_MASK) | if is_head { CALL_STACK_HEAD } else { 0 }
}

#[inline]
fn call_stack_ptr(word: u64) -> u64 {
    let ptr = word & CALL_STACK_PTR_MASK;
    if ptr & CALL_STACK_SIGN_BIT != 0 {
        ptr | CALL_STACK_SIGN_EXT
    } else {
        ptr
    }
}

#[inline]
fn call_stack_is_head(word: u64) -> bool {
    (word & CALL_STACK_HEAD) != 0
}

pub unsafe fn init(reply_kva: u64) {
    crate::kernel::smp::debug_assert_kernel_lock_held();
    unsafe {
        core::ptr::write_bytes(reply_kva as *mut u8, 0, core::mem::size_of::<Reply>());
    }
}

pub unsafe fn prepare_receiver(reply_kva: u64, receiver: *mut Tcb) -> bool {
    if reply_kva == 0 || receiver.is_null() {
        return false;
    }
    unsafe { cancel_owner_for_receive_if_needed(reply_kva, receiver) };
    true
}

pub unsafe fn bind_blocked_receiver(reply_kva: u64, receiver: *mut Tcb) -> bool {
    if reply_kva == 0 || receiver.is_null() {
        return false;
    }
    debug_assert!(
        tcb::blocked_on_receive_snapshot(receiver),
        "receiveIPC must block the receiver before binding its reply object",
    );
    let reply = reply_kva as *mut Reply;
    unsafe {
        let _guard = lock(reply_kva);
        (*reply).tcb = receiver as u64;
        (*reply).prev = 0;
        (*reply).next = 0;
        (*reply).can_grant = 0;
    }
    true
}

pub unsafe fn cancel_owner_for_receive_if_needed(reply_kva: u64, receiver: *mut Tcb) {
    if reply_kva == 0 {
        return;
    }
    let owner = unsafe {
        let _guard = lock(reply_kva);
        (*(reply_kva as *mut Reply)).tcb as *mut Tcb
    };
    if owner.is_null() || owner == receiver || !is_kernel_pspace_kva(owner as u64) {
        return;
    }
    unsafe {
        cancel_owner_for_receive(reply_kva, owner);
    }
}

unsafe fn cancel_blocked_endpoint_ipc(tcb: *mut Tcb, endpoint: u64) {
    let ep = endpoint as *mut crate::object::endpoint::Endpoint;
    unsafe {
        let _guard = crate::object::endpoint::lock_queue(ep);
        crate::object::endpoint::remove_waiter_locked(ep, tcb);
        tcb::cancel_ipc(tcb);
    }
}

unsafe fn cancel_blocked_receive_with_reply(reply_kva: u64, tcb: *mut Tcb, endpoint: u64) {
    let ep = endpoint as *mut crate::object::endpoint::Endpoint;
    unsafe {
        {
            let _guard = crate::object::endpoint::lock_queue(ep);
            crate::object::endpoint::remove_waiter_locked(ep, tcb);
        }
        // seL4 `cancelIPC(BlockedOnReceive)` unlinks the reply object before
        // the blocked receive state is made inactive.
        unlink(reply_kva, tcb);
        tcb::cancel_ipc(tcb);
    }
}

unsafe fn cancel_owner_for_receive(reply_kva: u64, tcb: *mut Tcb) {
    let (state, waiting_on) = tcb::wait_state_snapshot(tcb);
    unsafe {
        match state {
            state if state == tcb::ThreadState::BlockedOnSend as u8 => {
                cancel_blocked_endpoint_ipc(tcb, waiting_on);
            }
            state if state == tcb::ThreadState::BlockedOnReply as u8 => {
                remove_tcb(tcb);
            }
            state if state == tcb::ThreadState::BlockedOnReceive as u8 => {
                cancel_blocked_receive_with_reply(reply_kva, tcb, waiting_on);
            }
            _ => {
                let _guard = lock(reply_kva);
                clear_locked(reply_kva as *mut Reply);
            }
        }
    }
}

unsafe fn cancel_owner(reply_kva: u64) {
    if reply_kva == 0 {
        return;
    }
    let reply = reply_kva as *mut Reply;
    let tcb = unsafe {
        let _guard = lock(reply_kva);
        let tcb = (*reply).tcb as *mut Tcb;
        if tcb.is_null() {
            clear_locked(reply);
            return;
        }
        if !is_kernel_pspace_kva(tcb as u64) {
            clear_locked(reply);
            return;
        }
        tcb
    };

    let (state, waiting_on) = tcb::wait_state_snapshot(tcb);
    unsafe {
        match state {
            state if state == tcb::ThreadState::BlockedOnReply as u8 => {
                remove(reply_kva, tcb);
            }
            state if state == tcb::ThreadState::BlockedOnReceive as u8 => {
                cancel_blocked_receive_with_reply(reply_kva, tcb, waiting_on);
            }
            invalid_state => panic!(
                "finaliseCap(Reply): invalid owner thread state {}",
                invalid_state,
            ),
        }
    }
}

pub unsafe fn unbind_receiver(tcb: *mut Tcb) {
    if tcb.is_null() {
        return;
    }
    unsafe {
        let reply_kva = tcb::receive_reply_object_snapshot(tcb);
        if reply_kva == 0 || !is_kernel_pspace_kva(reply_kva) {
            return;
        }
        let reply = reply_kva as *mut Reply;
        let _guard = lock(reply_kva);
        if (*reply).tcb == tcb as u64 {
            clear_locked(reply);
        }
        tcb::clear_reply_binding_if(tcb, reply_kva);
    }
}

unsafe fn unlink_locked(reply_kva: u64, reply: *mut Reply, tcb: *mut Tcb) {
    unsafe {
        if (*reply).tcb == tcb as u64 {
            (*reply).tcb = 0;
        }
        tcb::clear_reply_binding_if(tcb, reply_kva);
        let slot = tcb::reply_slot_snapshot(tcb) as *mut Cte;
        if !slot.is_null() {
            let cap = cnode::cap_snapshot(slot);
            if cap.tag() == Some(CapTag::Reply)
                && cap.reply_is_object()
                && cap.reply_object_ptr() == reply_kva
            {
                tcb::clear_reply_slot_if(tcb, slot as u64);
            }
        }
        tcb::set_inactive(tcb);
    }
}

pub unsafe fn unlink(reply_kva: u64, tcb: *mut Tcb) {
    if reply_kva == 0 || tcb.is_null() {
        return;
    }
    let _guard = lock(reply_kva);
    unsafe {
        unlink_locked(reply_kva, reply_kva as *mut Reply, tcb);
    }
}

pub unsafe fn push(
    caller: *mut Tcb,
    callee: *mut Tcb,
    reply_kva: u64,
    _reply_rights: bool,
    can_grant: bool,
) -> bool {
    if caller.is_null() || callee.is_null() || reply_kva == 0 {
        return false;
    }
    let reply = reply_kva as *mut Reply;
    unsafe {
        let _guard = lock(reply_kva);
        let callee_blocked_receive = tcb::blocked_on_receive_snapshot(callee);
        if (*reply).tcb == callee as u64 && callee_blocked_receive {
            unlink_locked(reply_kva, reply, callee);
        }

        (*reply).tcb = caller as u64;
        (*reply).can_grant = can_grant as u64;
        (*reply).prev = 0;
        (*reply).next = 0;

        tcb::dequeue(caller);

        let _ = callee;
        tcb::set_blocked_on_reply(caller, reply_kva);
    }
    true
}

pub unsafe fn remove(reply_kva: u64, tcb: *mut Tcb) {
    if reply_kva == 0 || tcb.is_null() {
        return;
    }
    let reply = reply_kva as *mut Reply;
    unsafe {
        let _guard = lock(reply_kva);
        if (*reply).tcb != tcb as u64 {
            return;
        }

        let next = call_stack_ptr((*reply).next);
        let prev = call_stack_ptr((*reply).prev);
        {
            let _stack_guard = lock_call_stack();
            if next != 0 && call_stack_is_head((*reply).next) {
                if prev != 0 {
                    (*(prev as *mut Reply)).next = call_stack_new(next, true);
                }
                (*reply).next = 0;
            } else {
                if next != 0 {
                    (*(next as *mut Reply)).prev = 0;
                }
                if prev != 0 {
                    (*(prev as *mut Reply)).next = 0;
                }
            }
        }

        (*reply).prev = 0;
        (*reply).next = 0;
        unlink_locked(reply_kva, reply, tcb);
    }
}

pub unsafe fn remove_tcb(tcb: *mut Tcb) {
    if tcb.is_null() {
        return;
    }
    unsafe {
        let reply_kva = tcb::reply_object_snapshot(tcb);
        if reply_kva != 0 && is_kernel_pspace_kva(reply_kva) {
            let reply = reply_kva as *mut Reply;
            let _guard = lock(reply_kva);
            if (*reply).tcb == tcb as u64 {
                let next = call_stack_ptr((*reply).next);
                let prev = call_stack_ptr((*reply).prev);
                {
                    let _stack_guard = lock_call_stack();
                    if next != 0 {
                        if !call_stack_is_head((*reply).next) {
                            (*(next as *mut Reply)).prev = 0;
                        }
                    }
                    if prev != 0 {
                        (*(prev as *mut Reply)).next = 0;
                    }
                }
                (*reply).prev = 0;
                (*reply).next = 0;
                unlink_locked(reply_kva, reply, tcb);
            } else {
                tcb::clear_reply_binding_if(tcb, reply_kva);
            }
        }
        let reply_slot = tcb::reply_slot_snapshot(tcb);
        tcb::clear_reply_slot_if(tcb, reply_slot as u64);
    }
}

pub unsafe fn finalize(reply_kva: u64) {
    crate::kernel::smp::debug_assert_kernel_lock_held();
    unsafe {
        cancel_owner(reply_kva);
    }
}

pub unsafe fn tcb(reply_kva: u64) -> *mut Tcb {
    if reply_kva == 0 {
        return core::ptr::null_mut();
    }
    let reply = reply_kva as *mut Reply;
    unsafe {
        let _guard = lock(reply_kva);
        (*reply).tcb as *mut Tcb
    }
}

pub unsafe fn clear_next(reply_kva: u64) {
    if reply_kva == 0 {
        return;
    }
    let reply = reply_kva as *mut Reply;
    unsafe {
        let _guard = lock(reply_kva);
        let _stack_guard = lock_call_stack();
        debug_assert!(
            call_stack_is_head((*reply).next),
            "schedContext_unbindReply expected reply stack head",
        );
        (*reply).next = 0;
    }
}
