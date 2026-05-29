//! Minimal IRQ handler bookkeeping.
//!
//! seL4 stores IRQ notification bindings in an internal CNode
//! (`intStateIRQNode`) and tracks whether an IRQ handler cap has already
//! been issued. This module mirrors just enough of that model for
//! `IRQControl_Get`, `IRQHandler_SetNotification`, `Ack`, and `Clear`.

#![allow(dead_code)]

use core::cell::UnsafeCell;

use crate::object::cap::{Cap, CapTag};
use crate::object::cnode::Cte;
use crate::object::mdb::MdbNode;

pub const PLIC_MAX_IRQ: usize = 95;
pub const KERNEL_TIMER_IRQ: usize = PLIC_MAX_IRQ + 1;
pub const MAX_IRQ: usize = KERNEL_TIMER_IRQ;
const NUM_IRQS: usize = MAX_IRQ + 1;

#[derive(Copy, Clone)]
struct IrqEntry {
    active: bool,
    notification_slot: Cte,
}

const EMPTY_ENTRY: IrqEntry = IrqEntry {
    active: false,
    notification_slot: Cte {
        cap: Cap::null(),
        mdb: MdbNode::NULL,
    },
};

struct IrqTable(UnsafeCell<[IrqEntry; NUM_IRQS]>);
unsafe impl Sync for IrqTable {}

static IRQ_TABLE: IrqTable = IrqTable(UnsafeCell::new([EMPTY_ENTRY; NUM_IRQS]));

#[inline]
pub fn valid_irq(irq: u64) -> bool {
    irq > 0 && irq <= MAX_IRQ as u64
}

#[inline]
unsafe fn entry_mut(irq: u64) -> Option<&'static mut IrqEntry> {
    if !valid_irq(irq) {
        return None;
    }
    unsafe { Some(&mut (*IRQ_TABLE.0.get())[irq as usize]) }
}

pub unsafe fn is_active(irq: u64) -> bool {
    unsafe { entry_mut(irq).map(|e| e.active).unwrap_or(false) }
}

pub unsafe fn issue_handler(irq: u64) {
    if let Some(e) = unsafe { entry_mut(irq) } {
        e.active = true;
    }
}

pub unsafe fn delete_handler(irq: u64) {
    if let Some(e) = unsafe { entry_mut(irq) } {
        unsafe { clear_notification_slot(e) };
        e.active = false;
    }
}

pub unsafe fn set_notification(irq: u64, ntfn_cap: Cap, ntfn_slot: *mut Cte) {
    if ntfn_cap.tag() != Some(CapTag::Notification) || ntfn_slot.is_null() {
        return;
    }
    if let Some(e) = unsafe { entry_mut(irq) } {
        unsafe { clear_notification_slot(e) };
        e.notification_slot.cap = ntfn_cap;
        e.notification_slot.mdb = MdbNode::new(0, 0, false, false);
        unsafe {
            crate::object::cnode::mdb_insert_after(ntfn_slot, &mut e.notification_slot as *mut Cte);
        }
    }
}

pub unsafe fn clear_notification(irq: u64) {
    if let Some(e) = unsafe { entry_mut(irq) } {
        unsafe { clear_notification_slot(e) };
    }
}

unsafe fn clear_notification_slot(e: &mut IrqEntry) {
    if !e.notification_slot.cap.is_null() {
        unsafe { crate::object::cnode::mdb_unlink(&mut e.notification_slot as *mut Cte) };
        e.notification_slot.cap = Cap::null();
        e.notification_slot.mdb = MdbNode::NULL;
    }
}

pub unsafe fn signal_irq(irq: u64) {
    if let Some(e) = unsafe { entry_mut(irq) } {
        let cap = e.notification_slot.cap;
        if cap.tag() == Some(CapTag::Notification) && cap.notification_can_send() {
            let ntfn = cap.notification_ptr() as *mut crate::object::notification::Notification;
            unsafe { crate::object::notification::signal(ntfn, cap.notification_badge()) };
        }
    }
}
