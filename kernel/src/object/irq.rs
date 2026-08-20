//! Minimal IRQ handler bookkeeping.
//!
//! seL4 stores IRQ notification bindings in an internal CNode
//! (`intStateIRQNode`) and tracks whether an IRQ handler cap has already
//! been issued. This module mirrors just enough of that model for
//! `IRQControl_Get`, `IRQHandler_SetNotification`, `Ack`, and `Clear`.

#![allow(dead_code)]

use crate::kernel::smp::BklCell;
use crate::ktypes::objref::{ObjArray, ObjCell};
use crate::object::cap::{Cap, CapTag};
use crate::object::cnode::{Cte, CteRef};

pub const KERNEL_TIMER_IRQ: usize = crate::arch::current::object::interrupt::KERNEL_TIMER_IRQ;
pub const MAX_IRQ: usize = crate::arch::current::object::interrupt::MAX_IRQ;
const NUM_IRQS: usize = MAX_IRQ + 1;

/// seL4's `intStateIRQNode`: a kernel-owned CNode with one slot per IRQ,
/// holding the notification cap bound to that IRQ. It is a real CNode rather
/// than a plain array so that the ordinary capability machinery (derivation,
/// delete) can operate on its slots.
static IRQ_NODE: ObjCell<[Cte; NUM_IRQS]> = ObjCell::new([Cte::null(); NUM_IRQS]);

/// Whether a handler cap has been issued for each IRQ.
static IRQ_ACTIVE: BklCell<[bool; NUM_IRQS]> = BklCell::new([false; NUM_IRQS]);

#[inline]
pub fn valid_irq(irq: u64) -> bool {
    irq > 0 && irq <= MAX_IRQ as u64
}

/// The IRQ node viewed as a capability table.
fn irq_node() -> ObjArray<Cte> {
    // SAFETY: the static holds exactly `NUM_IRQS` contiguous CTEs, lives for
    // the whole run, and is only reached through this view.
    let base = unsafe { IRQ_NODE.get().cast::<Cte>() };
    // SAFETY: as above.
    unsafe { ObjArray::new(base, NUM_IRQS) }
}

/// The notification slot reserved for `irq`.
fn notification_slot(irq: u64) -> Option<CteRef> {
    if !valid_irq(irq) {
        return None;
    }
    irq_node().get(irq as usize)
}

fn is_active_irq(irq: u64) -> bool {
    valid_irq(irq) && IRQ_ACTIVE.with_ref(|active| active[irq as usize])
}

pub fn is_active(irq: u64) -> bool {
    is_active_irq(irq)
}

pub fn try_issue_handler(irq: u64) -> bool {
    if !valid_irq(irq) || is_active_irq(irq) {
        return false;
    }
    IRQ_ACTIVE.with_mut(|active| active[irq as usize] = true);
    crate::arch::current::object::interrupt::enable_external_irq(irq);
    true
}

pub fn deleting_handler(irq: u64) {
    crate::kernel::smp::debug_assert_kernel_lock_held();
    clear_notification(irq);
}

pub fn deleted_handler(irq: u64) {
    crate::kernel::smp::debug_assert_kernel_lock_held();
    if !valid_irq(irq) {
        return;
    }
    IRQ_ACTIVE.with_mut(|active| active[irq as usize] = false);
    crate::arch::current::object::interrupt::disable_external_irq(irq);
}

pub fn delete_handler(irq: u64) {
    deleting_handler(irq);
    deleted_handler(irq);
}

/// Bind `irq` to the notification named by `ntfn_slot`, replacing any existing
/// binding. Only a live handler cap may be bound.
pub fn set_notification(irq: u64, ntfn_cap: Cap, ntfn_slot: CteRef) -> bool {
    if ntfn_cap.tag() != Some(CapTag::Notification) || !is_active_irq(irq) {
        return false;
    }
    let Some(dest) = notification_slot(irq) else {
        return false;
    };
    if !same_notification_send_cap(ntfn_slot.cap(), ntfn_cap) {
        return false;
    }
    clear_notification(irq);
    let current_cap = ntfn_slot.cap();
    if !same_notification_send_cap(current_cap, ntfn_cap) {
        return false;
    }
    ntfn_slot.cte_insert(current_cap, dest);
    true
}

pub fn clear_notification(irq: u64) {
    if let Some(slot) = notification_slot(irq)
        && !slot.cap().is_null()
    {
        crate::api::invocation::cte_delete_one(slot);
    }
}

fn same_notification_send_cap(current: Cap, expected: Cap) -> bool {
    current.tag() == Some(CapTag::Notification)
        && current.notification_can_send()
        && current.notification_ptr() == expected.notification_ptr()
        && current.notification_badge() == expected.notification_badge()
}

/// Deliver `irq` to whichever notification is bound to it, reporting whether
/// there was one.
pub fn signal_irq(irq: u64) -> bool {
    let cap = notification_slot(irq).map_or(Cap::null(), CteRef::cap);
    let Some(ntfn) = cap
        .as_notification()
        .filter(|_| cap.notification_can_send())
    else {
        return false;
    };
    crate::arch::current::object::interrupt::complete_external_irq(irq);
    ntfn.signal(cap.notification_badge());
    true
}

pub fn ack_irq(irq: u64) {
    crate::arch::current::object::interrupt::complete_external_irq(irq);
}
