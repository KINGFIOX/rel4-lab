//! Minimal IRQ handler bookkeeping.
//!
//! seL4 stores IRQ notification bindings in an internal CNode
//! (`intStateIRQNode`) and tracks whether an IRQ handler cap has already
//! been issued. This module mirrors just enough of that model for
//! `IRQControl_Get`, `IRQHandler_SetNotification`, `Ack`, and `Clear`.

#![allow(dead_code)]

use crate::kernel::smp::BklCell;
use crate::object::cap::{Cap, CapTag};
use crate::object::cnode::Cte;
use crate::object::mdb::MdbNode;

pub const KERNEL_TIMER_IRQ: usize = crate::arch::current::irq::KERNEL_TIMER_IRQ;
pub const MAX_IRQ: usize = crate::arch::current::irq::MAX_IRQ;
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

static IRQ_TABLE: BklCell<[IrqEntry; NUM_IRQS]> = BklCell::new([EMPTY_ENTRY; NUM_IRQS]);

#[inline]
pub fn valid_irq(irq: u64) -> bool {
    irq > 0 && irq <= MAX_IRQ as u64
}

#[inline]
fn entry_mut(table: &mut [IrqEntry; NUM_IRQS], irq: u64) -> Option<&mut IrqEntry> {
    if !valid_irq(irq) {
        return None;
    }
    Some(&mut table[irq as usize])
}

pub unsafe fn is_active(irq: u64) -> bool {
    IRQ_TABLE.with_mut(|table| entry_mut(table, irq).map(|e| e.active).unwrap_or(false))
}

pub unsafe fn try_issue_handler(irq: u64) -> bool {
    IRQ_TABLE.with_mut(|table| {
        if let Some(e) = entry_mut(table, irq) {
            if e.active {
                return false;
            }
            e.active = true;
            crate::arch::current::irq::enable_external_irq(irq);
            return true;
        }
        false
    })
}

pub unsafe fn deleting_handler(irq: u64) {
    crate::kernel::smp::debug_assert_kernel_lock_held();
    IRQ_TABLE.with_mut(|table| {
        if let Some(e) = entry_mut(table, irq) {
            unsafe { clear_notification_slot(e) };
        }
    });
}

pub unsafe fn deleted_handler(irq: u64) {
    crate::kernel::smp::debug_assert_kernel_lock_held();
    IRQ_TABLE.with_mut(|table| {
        if let Some(e) = entry_mut(table, irq) {
            e.active = false;
            crate::arch::current::irq::disable_external_irq(irq);
        }
    });
}

pub unsafe fn delete_handler(irq: u64) {
    unsafe {
        deleting_handler(irq);
        deleted_handler(irq);
    }
}

pub unsafe fn set_notification(irq: u64, ntfn_cap: Cap, ntfn_slot: *mut Cte) -> bool {
    if ntfn_cap.tag() != Some(CapTag::Notification) || ntfn_slot.is_null() {
        return false;
    }
    IRQ_TABLE.with_mut(|table| {
        if let Some(e) = entry_mut(table, irq) {
            if e.active {
                unsafe {
                    let current_cap = {
                        let _cspace_guard = crate::object::cnode::lock_cspace();
                        (*ntfn_slot).cap
                    };
                    if !same_notification_send_cap(current_cap, ntfn_cap) {
                        return false;
                    }
                    clear_notification_slot(e);
                    let cspace_guard = crate::object::cnode::lock_cspace();
                    let current_cap = (*ntfn_slot).cap;
                    if !same_notification_send_cap(current_cap, ntfn_cap) {
                        return false;
                    }
                    crate::object::cnode::cte_insert_locked(
                        &cspace_guard,
                        current_cap,
                        ntfn_slot,
                        &mut e.notification_slot as *mut Cte,
                    );
                }
                return true;
            }
        }
        false
    })
}

pub unsafe fn clear_notification(irq: u64) {
    IRQ_TABLE.with_mut(|table| {
        if let Some(e) = entry_mut(table, irq) {
            unsafe { clear_notification_slot(e) };
        }
    });
}

fn same_notification_send_cap(current: Cap, expected: Cap) -> bool {
    current.tag() == Some(CapTag::Notification)
        && current.notification_can_send()
        && current.notification_ptr() == expected.notification_ptr()
        && current.notification_badge() == expected.notification_badge()
}

unsafe fn clear_notification_slot(e: &mut IrqEntry) {
    if !e.notification_slot.cap.is_null() {
        crate::api::invocation::cte_delete_one(&mut e.notification_slot as *mut Cte);
    }
}

pub unsafe fn signal_irq(irq: u64) -> bool {
    let cap = IRQ_TABLE.with_mut(|table| {
        if let Some(e) = entry_mut(table, irq) {
            e.notification_slot.cap
        } else {
            Cap::null()
        }
    });
    if cap.tag() == Some(CapTag::Notification) && cap.notification_can_send() {
        let ntfn = cap.notification_ptr() as *mut crate::object::notification::Notification;
        unsafe { crate::object::notification::signal(ntfn, cap.notification_badge()) };
        return true;
    }
    false
}

pub unsafe fn ack_irq(irq: u64) {
    crate::arch::current::irq::complete_external_irq(irq);
}
