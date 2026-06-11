//! Kernel-side TCB object.
//!
//! Lives in the 2 KiB (`seL4_TCBBits = 11`) region the user retypes from
//! an Untyped via `Untyped_Retype(seL4_TCBObject)`. Because that region
//! is always `2 KiB`-aligned and bigger than `size_of::<Tcb>()`, we can
//! safely treat the cap's pointer as a `*mut Tcb`.
//!
//! The scheduler is deliberately small: runnable TCBs live in per-core,
//! priority-indexed ready queues, and affinity selects the queue a TCB can
//! run on. A temporary big kernel lock still serialises most shared kernel state
//! while the SMP path matures.
//!
//! Layout-load: every field must fit comfortably inside the 2 KiB slab
//! the C kernel allocates, so the future C/Rust ABI swap stays valid.

#![allow(dead_code)]

use core::ptr::null_mut;

use crate::abi::constants::MAX_NUM_NODES;
use crate::arch::riscv64::trap::UserContext;
use crate::kernel::smp::{BklCell, BklObjectGuard};
use crate::object::cap::Cap;
use crate::object::cnode::Cte;

/// Pointer to the currently-scheduled TCB for the local hart.
#[inline]
pub fn current() -> *mut Tcb {
    crate::kernel::smp::current_tcb()
}

/// Replace the local hart's current TCB. Returns the previous pointer. Also
/// refreshes the legacy `api::thread` syscall view so cap lookups and IPC
/// accesses in the syscall slow path follow whichever TCB the scheduler last
/// picked.
#[inline]
pub fn set_current(tcb: *mut Tcb) -> *mut Tcb {
    let prev = crate::kernel::smp::set_current_tcb(tcb);
    unsafe { crate::api::thread::refresh_from_tcb(tcb) };
    prev
}

// ---- Ready-queue runqueues ------------------------------------------------
//
// `CONFIG_NUM_PRIORITIES = 256` per `kernel/include/configurations/gen_config.h`.
// One doubly-linked list per priority, all backed by `Tcb.queue_{next,prev}`
// (stored as raw u64 ptrs because `Tcb` lives in user-controlled memory
// and we want the field offsets to stay byte-identical across the C ↔ Rust
// boundary).
//
// User execution can now happen on more than one hart. Queue links, bitmaps,
// and surrounding TCB/SC state transitions are serialized by the seL4-style
// big kernel lock.

pub const NUM_PRIORITIES: usize = 256;
pub const DEFAULT_TIME_SLICE_TICKS: u8 = 5;
pub const TCB_CNODE_RADIX: usize = 4;
pub const TCB_CNODE_ENTRIES: usize = 1 << TCB_CNODE_RADIX;
pub const TCB_ARCH_CNODE_ENTRIES: usize = TCB_CNODE_ENTRIES;
pub const TCB_CTABLE_SLOT: usize = 0;
pub const TCB_VTABLE_SLOT: usize = 1;
pub const TCB_BUFFER_SLOT: usize = 2;
pub const TCB_FAULT_HANDLER_SLOT: usize = 3;
pub const TCB_TIMEOUT_HANDLER_SLOT: usize = 4;
pub(crate) const TCB_SENDER_EXTRA_CAPS: usize = 3;

#[repr(C)]
#[derive(Copy, Clone)]
struct Queue {
    head: *mut Tcb,
    tail: *mut Tcb,
}

impl Queue {
    const fn empty() -> Self {
        Self {
            head: null_mut(),
            tail: null_mut(),
        }
    }
}

struct RunqueueCore {
    queues: [Queue; NUM_PRIORITIES],
    ready_bitmap: [u64; 4],
}

// Runqueue entries are kernel-owned TCB pointers, and all queue mutation is
// serialized by the BKL.
unsafe impl Send for RunqueueCore {}

impl RunqueueCore {
    const fn new() -> Self {
        Self {
            queues: [const { Queue::empty() }; NUM_PRIORITIES],
            ready_bitmap: [0; 4],
        }
    }

    fn queue_mut(&mut self, prio: usize) -> &mut Queue {
        debug_assert!(prio < NUM_PRIORITIES);
        &mut self.queues[prio]
    }

    fn queue(&self, prio: usize) -> &Queue {
        debug_assert!(prio < NUM_PRIORITIES);
        &self.queues[prio]
    }

    fn set_ready_bit(&mut self, prio: usize) {
        self.ready_bitmap[prio / 64] |= 1u64 << (prio % 64);
    }

    fn clear_ready_bit(&mut self, prio: usize) {
        self.ready_bitmap[prio / 64] &= !(1u64 << (prio % 64));
    }

    unsafe fn enqueue_tail(&mut self, prio: usize, tcb: *mut Tcb) -> bool {
        let inserted = unsafe { enqueue_tail_locked(self.queue_mut(prio), tcb) };
        if inserted {
            self.set_ready_bit(prio);
        }
        inserted
    }

    unsafe fn enqueue_head(&mut self, prio: usize, tcb: *mut Tcb) -> bool {
        let inserted = unsafe { enqueue_head_locked(self.queue_mut(prio), tcb) };
        if inserted {
            self.set_ready_bit(prio);
        }
        inserted
    }

    unsafe fn dequeue_from_priority(&mut self, prio: usize, tcb: *mut Tcb) -> bool {
        let became_empty = {
            let queue = self.queue_mut(prio);
            unsafe {
                if !ready_queue_contains(queue, tcb) {
                    return false;
                }
                unlink_from_ready_queue(queue, tcb)
            }
        };
        if became_empty {
            self.clear_ready_bit(prio);
        }
        true
    }

    fn ready_bits(&self, word_idx: usize) -> u64 {
        self.ready_bitmap[word_idx]
    }
}

static RUNQUEUES: [BklCell<RunqueueCore>; MAX_NUM_NODES] =
    [const { BklCell::new(RunqueueCore::new()) }; MAX_NUM_NODES];

pub(crate) type TcbStateLockGuard = BklObjectGuard;

#[inline]
pub(crate) fn lock_state(_tcb: *const Tcb) -> TcbStateLockGuard {
    BklObjectGuard::new()
}

#[derive(Copy, Clone)]
struct RunqueueSnapshot {
    core: usize,
    prio: usize,
    sched_context: u64,
    state: u8,
}

#[derive(Copy, Clone)]
pub(crate) struct FaultIpcMessage {
    pub label: u64,
    pub len: u64,
    pub mrs: [u64; 16],
}

#[derive(Copy, Clone)]
pub(crate) struct QueuedSenderSnapshot {
    pub info_word: u64,
    pub badge: u64,
    pub is_call: bool,
    pub can_grant: bool,
    pub can_grant_reply: bool,
    pub extra_cap_slots: [u64; TCB_SENDER_EXTRA_CAPS],
    pub is_fault: bool,
    pub fault_label: u64,
}

#[derive(Copy, Clone)]
pub(crate) struct ThreadViewSnapshot {
    pub cspace_cap: Cap,
    pub vspace_cap: Cap,
    pub ipc_buffer_kva: u64,
    pub ipc_buffer_uva: u64,
}

#[inline]
fn core_for_affinity(affinity: u8) -> usize {
    let core = affinity as usize;
    if core < MAX_NUM_NODES { core } else { 0 }
}

#[inline]
fn runqueue_snapshot(tcb: *const Tcb) -> RunqueueSnapshot {
    if tcb.is_null() {
        return RunqueueSnapshot {
            core: 0,
            prio: 0,
            sched_context: 0,
            state: ThreadState::Inactive as u8,
        };
    }
    let (affinity, priority, sched_context, state) = unsafe {
        let _guard = lock_state(tcb);
        (
            (*tcb).affinity,
            (*tcb).priority,
            (*tcb).sched_context,
            (*tcb).state,
        )
    };
    RunqueueSnapshot {
        core: core_for_affinity(affinity),
        prio: priority as usize,
        sched_context,
        state,
    }
}

#[inline]
pub(crate) fn priority_snapshot(tcb: *const Tcb) -> usize {
    if tcb.is_null() {
        return 0;
    }
    unsafe {
        let _guard = lock_state(tcb);
        (*tcb).priority as usize
    }
}

#[inline]
pub(crate) fn mcp_snapshot(tcb: *const Tcb) -> usize {
    if tcb.is_null() {
        return 0;
    }
    unsafe {
        let _guard = lock_state(tcb);
        (*tcb).mcp as usize
    }
}

#[inline]
pub(crate) fn yield_authority_snapshot(tcb: *const Tcb) -> (usize, u64, usize) {
    if tcb.is_null() {
        return (0, 0, 0);
    }
    unsafe {
        let _guard = lock_state(tcb);
        (
            (*tcb).mcp as usize,
            (*tcb).yield_to_sc,
            (*tcb).priority as usize,
        )
    }
}

#[inline]
pub(crate) fn fault_endpoint_snapshot(tcb: *const Tcb) -> Cap {
    if tcb.is_null() {
        return Cap::null();
    }
    unsafe {
        let _guard = lock_state(tcb);
        cap_slot_snapshot_locked(tcb, TCB_FAULT_HANDLER_SLOT)
    }
}

#[inline]
pub(crate) fn cspace_cap_snapshot(tcb: *const Tcb) -> Cap {
    if tcb.is_null() {
        return Cap::null();
    }
    unsafe {
        let _guard = lock_state(tcb);
        cap_slot_snapshot_locked(tcb, TCB_CTABLE_SLOT)
    }
}

#[inline]
pub(crate) fn vspace_cap_snapshot(tcb: *const Tcb) -> Cap {
    if tcb.is_null() {
        return Cap::null();
    }
    unsafe {
        let _guard = lock_state(tcb);
        cap_slot_snapshot_locked(tcb, TCB_VTABLE_SLOT)
    }
}

#[inline]
pub(crate) fn ipc_buffer_kva_snapshot(tcb: *const Tcb) -> u64 {
    if tcb.is_null() {
        return 0;
    }
    unsafe {
        let _guard = lock_state(tcb);
        (*tcb).ipc_buffer_kva
    }
}

#[inline]
pub(crate) fn thread_view_snapshot(tcb: *const Tcb) -> ThreadViewSnapshot {
    if tcb.is_null() {
        return ThreadViewSnapshot {
            cspace_cap: Cap::null(),
            vspace_cap: Cap::null(),
            ipc_buffer_kva: 0,
            ipc_buffer_uva: 0,
        };
    }
    unsafe {
        let _guard = lock_state(tcb);
        let cspace_cap = cap_slot_snapshot_locked(tcb, TCB_CTABLE_SLOT);
        let vspace_cap = cap_slot_snapshot_locked(tcb, TCB_VTABLE_SLOT);
        ThreadViewSnapshot {
            cspace_cap,
            vspace_cap,
            ipc_buffer_kva: (*tcb).ipc_buffer_kva,
            ipc_buffer_uva: (*tcb).ipc_buffer_uva,
        }
    }
}

#[inline]
pub(crate) fn timeout_endpoint_snapshot(tcb: *const Tcb) -> Cap {
    if tcb.is_null() {
        return Cap::null();
    }
    unsafe {
        let _guard = lock_state(tcb);
        cap_slot_snapshot_locked(tcb, TCB_TIMEOUT_HANDLER_SLOT)
    }
}

#[inline]
pub(crate) fn sched_context_snapshot(tcb: *const Tcb) -> u64 {
    if tcb.is_null() {
        return 0;
    }
    unsafe {
        let _guard = lock_state(tcb);
        (*tcb).sched_context
    }
}

pub(crate) unsafe fn bind_sched_context_if_unbound(tcb: *mut Tcb, sched_context: u64) -> bool {
    if tcb.is_null() || sched_context == 0 {
        return false;
    }
    unsafe {
        let _guard = lock_state(tcb);
        if (*tcb).sched_context != 0 {
            return false;
        }
        (*tcb).sched_context = sched_context;
        true
    }
}

pub(crate) unsafe fn bind_sched_context_if_unbound_or_same(
    tcb: *mut Tcb,
    sched_context: u64,
) -> bool {
    if tcb.is_null() || sched_context == 0 {
        return false;
    }
    unsafe {
        let _guard = lock_state(tcb);
        if (*tcb).sched_context != 0 && (*tcb).sched_context != sched_context {
            return false;
        }
        (*tcb).sched_context = sched_context;
        true
    }
}

pub(crate) unsafe fn clear_sched_context_if(tcb: *mut Tcb, sched_context: u64) -> bool {
    if tcb.is_null() {
        return false;
    }
    unsafe {
        let _guard = lock_state(tcb);
        if (*tcb).sched_context != sched_context {
            return false;
        }
        (*tcb).sched_context = 0;
        true
    }
}

/// TCB pair updates are serialized by the seL4-style BKL, not by ordered
/// per-TCB locks. The pointers stay in the helper signature to document the
/// pair being mutated.
fn with_tcb_pair_guard<R>(first: *const Tcb, second: *const Tcb, f: impl FnOnce() -> R) -> R {
    let _ = (first, second);
    let _guard = BklObjectGuard::new();
    f()
}

pub(crate) unsafe fn move_sched_context_if_target_unbound(
    from: *mut Tcb,
    to: *mut Tcb,
    sched_context: u64,
) -> bool {
    if from.is_null() || to.is_null() || sched_context == 0 {
        return false;
    }
    if from == to {
        return false;
    }
    unsafe {
        with_tcb_pair_guard(from, to, || {
            if (*from).sched_context != sched_context || (*to).sched_context != 0 {
                return false;
            }
            (*from).sched_context = 0;
            (*to).sched_context = sched_context;
            true
        })
    }
}

#[inline]
pub(crate) fn yield_to_sched_context_snapshot(tcb: *const Tcb) -> u64 {
    if tcb.is_null() {
        return 0;
    }
    unsafe {
        let _guard = lock_state(tcb);
        (*tcb).yield_to_sc
    }
}

#[inline]
pub(crate) fn yield_from_snapshot(tcb: *const Tcb) -> *mut Tcb {
    if tcb.is_null() {
        return core::ptr::null_mut();
    }
    unsafe {
        let _guard = lock_state(tcb);
        (*tcb).yield_from_tcb as *mut Tcb
    }
}

pub(crate) unsafe fn start_yield_to_if_idle(
    yielder: *mut Tcb,
    target: *mut Tcb,
    sched_context: u64,
    consumed_start: u64,
) -> bool {
    if yielder.is_null() || target.is_null() || yielder == target || sched_context == 0 {
        return false;
    }
    unsafe {
        with_tcb_pair_guard(yielder, target, || {
            if (*yielder).yield_to_sc != 0 || (*target).yield_from_tcb != 0 {
                return false;
            }
            (*yielder).yield_to_sc = sched_context;
            (*yielder).yield_to_consumed_start = consumed_start;
            (*target).yield_from_tcb = yielder as u64;
            true
        })
    }
}

pub(crate) unsafe fn yield_to_pair_matches(
    yielder: *mut Tcb,
    target: *mut Tcb,
    sched_context: u64,
) -> bool {
    if yielder.is_null() || target.is_null() {
        return false;
    }
    unsafe {
        with_tcb_pair_guard(yielder, target, || {
            (*target).yield_from_tcb == yielder as u64 && (*yielder).yield_to_sc == sched_context
        })
    }
}

pub(crate) unsafe fn clear_yield_to(yielder: *mut Tcb) {
    if yielder.is_null() {
        return;
    }
    unsafe {
        let _guard = lock_state(yielder);
        (*yielder).yield_to_sc = 0;
        (*yielder).yield_to_consumed_start = 0;
    }
}

pub(crate) unsafe fn cancel_yield_to_for_target(yielder: *mut Tcb, target: *mut Tcb) {
    if yielder.is_null() {
        return;
    }
    if target.is_null() || yielder == target {
        unsafe {
            let _guard = lock_state(yielder);
            if (*yielder).yield_from_tcb == yielder as u64 {
                (*yielder).yield_from_tcb = 0;
            }
            (*yielder).yield_to_sc = 0;
            (*yielder).yield_to_consumed_start = 0;
        }
        return;
    }
    unsafe {
        with_tcb_pair_guard(yielder, target, || {
            if (*target).yield_from_tcb == yielder as u64 {
                (*target).yield_from_tcb = 0;
            }
            (*yielder).yield_to_sc = 0;
            (*yielder).yield_to_consumed_start = 0;
        });
    }
}

pub(crate) unsafe fn clear_yield_from_if(target: *mut Tcb, yielder: *mut Tcb) -> bool {
    if target.is_null() || yielder.is_null() {
        return false;
    }
    unsafe {
        let _guard = lock_state(target);
        if (*target).yield_from_tcb != yielder as u64 {
            return false;
        }
        (*target).yield_from_tcb = 0;
        true
    }
}

pub(crate) unsafe fn finish_yield_to(yielder: *mut Tcb, sched_context: u64, consumed: u64) -> bool {
    if yielder.is_null() || sched_context == 0 {
        return false;
    }
    unsafe {
        let _guard = lock_state(yielder);
        if (*yielder).yield_to_sc != sched_context {
            return false;
        }
        (*yielder).yield_to_sc = 0;
        (*yielder).yield_to_consumed_start = 0;
        (*yielder).context.regs[crate::arch::riscv64::trap::UserRegister::A2.index()] = consumed;
        (*yielder).state = ThreadState::Running as u8;
        true
    }
}

pub(crate) unsafe fn finish_yield_to_pair(
    yielder: *mut Tcb,
    target: *mut Tcb,
    sched_context: u64,
    consumed: u64,
) -> bool {
    if yielder.is_null() || target.is_null() {
        return false;
    }
    unsafe {
        with_tcb_pair_guard(yielder, target, || {
            if (*target).yield_from_tcb != yielder as u64 || (*yielder).yield_to_sc != sched_context
            {
                return false;
            }
            (*target).yield_from_tcb = 0;
            (*yielder).yield_to_sc = 0;
            (*yielder).yield_to_consumed_start = 0;
            (*yielder).context.regs[crate::arch::riscv64::trap::UserRegister::A2.index()] =
                consumed;
            (*yielder).state = ThreadState::Running as u8;
            true
        })
    }
}

#[inline]
pub(crate) fn running_sched_context_snapshot(tcb: *const Tcb) -> (bool, u64) {
    if tcb.is_null() {
        return (false, 0);
    }
    unsafe {
        let _guard = lock_state(tcb);
        (
            (*tcb).state == ThreadState::Running as u8,
            (*tcb).sched_context,
        )
    }
}

#[inline]
pub(crate) fn runnable_sched_context_snapshot(tcb: *const Tcb) -> (bool, u64) {
    if tcb.is_null() {
        return (false, 0);
    }
    unsafe {
        let _guard = lock_state(tcb);
        let state = (*tcb).state;
        (
            state == ThreadState::Running as u8 || state == ThreadState::Restart as u8,
            (*tcb).sched_context,
        )
    }
}

#[inline]
pub(crate) fn reply_slot_snapshot(tcb: *const Tcb) -> u64 {
    if tcb.is_null() {
        return 0;
    }
    unsafe {
        let _guard = lock_state(tcb);
        (*tcb).reply_slot
    }
}

#[inline]
pub(crate) fn reply_object_snapshot(tcb: *const Tcb) -> u64 {
    if tcb.is_null() {
        return 0;
    }
    unsafe {
        let _guard = lock_state(tcb);
        (*tcb).reply_object
    }
}

#[inline]
pub(crate) fn receive_reply_object_snapshot(tcb: *const Tcb) -> u64 {
    if tcb.is_null() {
        return 0;
    }
    unsafe {
        let _guard = lock_state(tcb);
        (*tcb).receive_reply_object
    }
}

#[inline]
pub(crate) fn receive_reply_snapshot(tcb: *const Tcb) -> (u64, u64, bool) {
    if tcb.is_null() {
        return (0, 0, false);
    }
    unsafe {
        let _guard = lock_state(tcb);
        (
            (*tcb).receive_reply_cptr,
            (*tcb).receive_reply_object,
            (*tcb).receive_reply_can_grant != 0,
        )
    }
}

#[inline]
pub(crate) fn wait_state_snapshot(tcb: *const Tcb) -> (u8, u64) {
    if tcb.is_null() {
        return (ThreadState::Inactive as u8, 0);
    }
    unsafe {
        let _guard = lock_state(tcb);
        ((*tcb).state, (*tcb).waiting_on)
    }
}

#[inline]
pub(crate) fn blocked_on_reply_snapshot(tcb: *const Tcb) -> bool {
    if tcb.is_null() {
        return false;
    }
    unsafe {
        let _guard = lock_state(tcb);
        (*tcb).state == ThreadState::BlockedOnReply as u8
    }
}

#[inline]
pub(crate) fn blocked_on_receive_snapshot(tcb: *const Tcb) -> bool {
    if tcb.is_null() {
        return false;
    }
    unsafe {
        let _guard = lock_state(tcb);
        (*tcb).state == ThreadState::BlockedOnReceive as u8
    }
}

#[inline]
pub(crate) fn waiter_matches_endpoint(tcb: *const Tcb, endpoint: u64, sending: bool) -> bool {
    if tcb.is_null() || endpoint == 0 {
        return false;
    }
    unsafe {
        let _guard = lock_state(tcb);
        if (*tcb).waiting_on != endpoint {
            return false;
        }
        let expected = if sending {
            ThreadState::BlockedOnSend
        } else {
            ThreadState::BlockedOnReceive
        };
        (*tcb).state == expected as u8
    }
}

#[inline]
pub(crate) fn blocked_on_notification_for(tcb: *const Tcb, notification: u64) -> bool {
    if tcb.is_null() || notification == 0 {
        return false;
    }
    unsafe {
        let _guard = lock_state(tcb);
        (*tcb).waiting_on == notification
            && (*tcb).state == ThreadState::BlockedOnNotification as u8
    }
}

#[inline]
pub(crate) fn blocked_receive_endpoint_snapshot(tcb: *const Tcb) -> Option<u64> {
    if tcb.is_null() {
        return None;
    }
    unsafe {
        let _guard = lock_state(tcb);
        if (*tcb).state == ThreadState::BlockedOnReceive as u8 {
            Some((*tcb).waiting_on)
        } else {
            None
        }
    }
}

#[inline]
pub(crate) fn sender_badge_snapshot(tcb: *const Tcb) -> u64 {
    if tcb.is_null() {
        return 0;
    }
    unsafe {
        let _guard = lock_state(tcb);
        (*tcb).sender_badge
    }
}

// Endpoint/Notification wait queues reuse the same TCB link fields as
// runqueues. Callers take wait-object/TCB marker guards to assert they are
// already inside the seL4-style BKL.
#[inline]
pub(crate) fn wait_queue_next_snapshot(tcb: *const Tcb) -> *mut Tcb {
    if tcb.is_null() {
        return core::ptr::null_mut();
    }
    unsafe {
        let _guard = lock_state(tcb);
        (*tcb).queue_next as *mut Tcb
    }
}

#[inline]
pub(crate) fn wait_queue_links_snapshot(tcb: *const Tcb) -> (*mut Tcb, *mut Tcb) {
    if tcb.is_null() {
        return (core::ptr::null_mut(), core::ptr::null_mut());
    }
    unsafe {
        let _guard = lock_state(tcb);
        ((*tcb).queue_prev as *mut Tcb, (*tcb).queue_next as *mut Tcb)
    }
}

#[inline]
pub(crate) unsafe fn set_wait_queue_next(tcb: *mut Tcb, next: *mut Tcb) {
    if tcb.is_null() {
        return;
    }
    unsafe {
        let _guard = lock_state(tcb);
        (*tcb).queue_next = next as u64;
    }
}

#[inline]
pub(crate) unsafe fn set_wait_queue_prev(tcb: *mut Tcb, prev: *mut Tcb) {
    if tcb.is_null() {
        return;
    }
    unsafe {
        let _guard = lock_state(tcb);
        (*tcb).queue_prev = prev as u64;
    }
}

#[inline]
pub(crate) unsafe fn set_wait_queue_links(tcb: *mut Tcb, prev: *mut Tcb, next: *mut Tcb) {
    if tcb.is_null() {
        return;
    }
    unsafe {
        let _guard = lock_state(tcb);
        (*tcb).queue_prev = prev as u64;
        (*tcb).queue_next = next as u64;
    }
}

#[inline]
pub(crate) unsafe fn clear_wait_queue_links(tcb: *mut Tcb) {
    if tcb.is_null() {
        return;
    }
    unsafe {
        let _guard = lock_state(tcb);
        (*tcb).queue_next = 0;
        (*tcb).queue_prev = 0;
    }
}

pub(crate) unsafe fn set_reply_slot_and_object(tcb: *mut Tcb, slot: u64, reply_object: u64) {
    if tcb.is_null() {
        return;
    }
    unsafe {
        let _guard = lock_state(tcb);
        (*tcb).reply_slot = slot;
        (*tcb).reply_object = reply_object;
    }
}

pub(crate) unsafe fn clear_reply_slot_if(tcb: *mut Tcb, slot: u64) -> bool {
    if tcb.is_null() {
        return false;
    }
    unsafe {
        let _guard = lock_state(tcb);
        if (*tcb).reply_slot != slot {
            return false;
        }
        (*tcb).reply_slot = 0;
        if slot != 0 {
            (*tcb).reply_object = 0;
        }
        true
    }
}

pub(crate) unsafe fn clear_reply_binding_if(tcb: *mut Tcb, reply_object: u64) {
    if tcb.is_null() || reply_object == 0 {
        return;
    }
    unsafe {
        let _guard = lock_state(tcb);
        if (*tcb).reply_object == reply_object {
            (*tcb).reply_object = 0;
            (*tcb).reply_slot = 0;
        }
        if (*tcb).receive_reply_object == reply_object {
            (*tcb).receive_reply_object = 0;
            (*tcb).receive_reply_cptr = 0;
            (*tcb).receive_reply_can_grant = 0;
        }
    }
}

pub(crate) unsafe fn set_inactive(tcb: *mut Tcb) {
    if tcb.is_null() {
        return;
    }
    unsafe {
        let _guard = lock_state(tcb);
        (*tcb).state = ThreadState::Inactive as u8;
    }
}

pub(crate) unsafe fn rewind_pc(tcb: *mut Tcb) {
    if tcb.is_null() {
        return;
    }
    unsafe {
        let _guard = lock_state(tcb);
        (*tcb).context.pc = (*tcb).context.pc.wrapping_sub(4);
        (*tcb).context.restart_pc = (*tcb).context.pc;
    }
}

pub(crate) unsafe fn clear_blocked_receive_state(tcb: *mut Tcb) {
    if tcb.is_null() {
        return;
    }
    unsafe {
        let _guard = lock_state(tcb);
        (*tcb).waiting_on = 0;
        clear_endpoint_ipc_state_locked(tcb);
    }
}

pub(crate) unsafe fn set_blocked_on_reply(tcb: *mut Tcb, reply_object: u64) -> u64 {
    if tcb.is_null() {
        return 0;
    }
    unsafe {
        let _guard = lock_state(tcb);
        (*tcb).waiting_on = 0;
        clear_endpoint_ipc_state_locked_preserving_fault(tcb);
        (*tcb).state = ThreadState::BlockedOnReply as u8;
        (*tcb).reply_object = reply_object;
        (*tcb).sched_context
    }
}

pub(crate) unsafe fn clear_waiting_on(tcb: *mut Tcb) {
    if tcb.is_null() {
        return;
    }
    unsafe {
        let _guard = lock_state(tcb);
        (*tcb).waiting_on = 0;
    }
}

/// Mark a selected runnable TCB active and return the saved user context that
/// the trap restore path should load.
pub(crate) unsafe fn prepare_for_user_restore(tcb: *mut Tcb) -> *mut UserContext {
    if tcb.is_null() {
        return null_mut();
    }
    unsafe {
        let _guard = lock_state(tcb);
        if (*tcb).state == ThreadState::Restart as u8 {
            (*tcb).state = ThreadState::Running as u8;
        }
        &raw mut (*tcb).context
    }
}

pub(crate) unsafe fn set_running_with_reply_regs(tcb: *mut Tcb, badge: u64, info_word: u64) {
    if tcb.is_null() {
        return;
    }
    unsafe {
        let _guard = lock_state(tcb);
        (*tcb).context.regs[crate::arch::riscv64::trap::UserRegister::A0.index()] = badge;
        (*tcb).context.regs[crate::arch::riscv64::trap::UserRegister::A1.index()] = info_word;
        (*tcb).state = ThreadState::Running as u8;
    }
}

pub(crate) unsafe fn set_blocked_on_notification(tcb: *mut Tcb, notification: u64) {
    if tcb.is_null() {
        return;
    }
    unsafe {
        let _guard = lock_state(tcb);
        (*tcb).state = ThreadState::BlockedOnNotification as u8;
        (*tcb).waiting_on = notification;
    }
}

pub(crate) unsafe fn complete_notification_wait(tcb: *mut Tcb, badge: u64) {
    if tcb.is_null() {
        return;
    }
    unsafe {
        let _guard = lock_state(tcb);
        (*tcb).waiting_on = 0;
        write_notification_badge_regs_locked(tcb, badge);
        (*tcb).state = ThreadState::Running as u8;
    }
}

pub(crate) unsafe fn complete_bound_endpoint_notification_wait(tcb: *mut Tcb, badge: u64) {
    if tcb.is_null() {
        return;
    }
    unsafe {
        crate::object::reply::unbind_receiver(tcb);
    }
    unsafe {
        let _guard = lock_state(tcb);
        (*tcb).waiting_on = 0;
        clear_endpoint_ipc_state_locked(tcb);
        write_notification_badge_regs_locked(tcb, badge);
        (*tcb).state = ThreadState::Running as u8;
    }
}

unsafe fn write_notification_badge_regs_locked(tcb: *mut Tcb, badge: u64) {
    unsafe {
        (*tcb).context.regs[crate::arch::riscv64::trap::UserRegister::A0.index()] = badge;
        (*tcb).context.regs[crate::arch::riscv64::trap::UserRegister::A1.index()] = 0;
        (*tcb).context.regs[crate::arch::riscv64::trap::UserRegister::A2.index()] = 0;
        (*tcb).context.regs[crate::arch::riscv64::trap::UserRegister::A3.index()] = 0;
        (*tcb).context.regs[crate::arch::riscv64::trap::UserRegister::A4.index()] = 0;
        (*tcb).context.regs[crate::arch::riscv64::trap::UserRegister::A5.index()] = 0;
    }
}

pub(crate) unsafe fn restart_endpoint_waiter(tcb: *mut Tcb) -> *mut Tcb {
    unsafe { cancel_endpoint_waiter(tcb, None).0 }
}

unsafe fn clear_endpoint_ipc_state_locked(tcb: *mut Tcb) {
    unsafe {
        (*tcb).receiver_can_grant = 0;
        (*tcb).receive_reply_cptr = 0;
        (*tcb).receive_reply_object = 0;
        (*tcb).receive_reply_can_grant = 0;
        (*tcb).sender_badge = 0;
        (*tcb).sender_can_grant = 0;
        (*tcb).sender_can_grant_reply = 0;
        (*tcb).sender_extra_cap_slots = [0; TCB_SENDER_EXTRA_CAPS];
        (*tcb).sender_is_call = 0;
        (*tcb).sender_is_fault = 0;
        (*tcb).fault_label = 0;
        (*tcb).fault_len = 0;
        (*tcb).fault_mrs = [0; 16];
    }
}

unsafe fn clear_endpoint_ipc_state_locked_preserving_fault(tcb: *mut Tcb) {
    unsafe {
        // Some transitions clear stale endpoint send/receive metadata while
        // leaving a pending fault to be consumed by the reply path. Explicit
        // `cancel_ipc` still clears the fault payload.
        let sender_is_fault = (*tcb).sender_is_fault;
        let fault_label = (*tcb).fault_label;
        let fault_len = (*tcb).fault_len;
        let fault_mrs = (*tcb).fault_mrs;
        clear_endpoint_ipc_state_locked(tcb);
        if sender_is_fault != 0 {
            (*tcb).sender_is_fault = sender_is_fault;
            (*tcb).fault_label = fault_label;
            (*tcb).fault_len = fault_len;
            (*tcb).fault_mrs = fault_mrs;
        }
    }
}

pub(crate) unsafe fn cancel_ipc(tcb: *mut Tcb) {
    if tcb.is_null() {
        return;
    }
    unsafe {
        let _guard = lock_state(tcb);
        (*tcb).waiting_on = 0;
        clear_endpoint_ipc_state_locked(tcb);
        (*tcb).state = ThreadState::Inactive as u8;
    }
}

/// Remove `tcb` from a detached endpoint wait list and return the next waiter
/// plus whether this waiter should be made runnable.
pub(crate) unsafe fn cancel_endpoint_waiter(
    tcb: *mut Tcb,
    call_error_info: Option<u64>,
) -> (*mut Tcb, bool) {
    if tcb.is_null() {
        return (core::ptr::null_mut(), false);
    }
    let state = unsafe {
        let _guard = lock_state(tcb);
        (*tcb).state
    };
    if state == ThreadState::BlockedOnReceive as u8 {
        unsafe {
            crate::object::reply::unbind_receiver(tcb);
        }
    }
    unsafe {
        let _guard = lock_state(tcb);
        let next = (*tcb).queue_next as *mut Tcb;
        let was_call = (*tcb).sender_is_call != 0;
        let was_fault_sender =
            (*tcb).state == ThreadState::BlockedOnSend as u8 && (*tcb).sender_is_fault != 0;
        let preserve_fault = was_fault_sender && call_error_info.is_none();

        (*tcb).queue_next = 0;
        (*tcb).queue_prev = 0;
        (*tcb).waiting_on = 0;
        if preserve_fault {
            clear_endpoint_ipc_state_locked_preserving_fault(tcb);
        } else {
            clear_endpoint_ipc_state_locked(tcb);
        }

        if let Some(info_word) = call_error_info {
            (*tcb).context.regs[crate::arch::riscv64::trap::UserRegister::A0.index()] = 0;
            if was_call {
                (*tcb).context.regs[crate::arch::riscv64::trap::UserRegister::A1.index()] =
                    info_word;
                (*tcb).context.regs[crate::arch::riscv64::trap::UserRegister::A2.index()] = 0;
                (*tcb).context.regs[crate::arch::riscv64::trap::UserRegister::A3.index()] = 0;
                (*tcb).context.regs[crate::arch::riscv64::trap::UserRegister::A4.index()] = 0;
                (*tcb).context.regs[crate::arch::riscv64::trap::UserRegister::A5.index()] = 0;
            }
        }

        if was_fault_sender {
            (*tcb).state = ThreadState::Inactive as u8;
            (next, false)
        } else {
            (*tcb).state = ThreadState::Restart as u8;
            (next, true)
        }
    }
}

pub(crate) unsafe fn restart_notification_waiter(tcb: *mut Tcb) -> *mut Tcb {
    if tcb.is_null() {
        return core::ptr::null_mut();
    }
    unsafe {
        let _guard = lock_state(tcb);
        let next = (*tcb).queue_next as *mut Tcb;
        (*tcb).queue_next = 0;
        (*tcb).queue_prev = 0;
        (*tcb).waiting_on = 0;
        (*tcb).state = ThreadState::Restart as u8;
        next
    }
}

pub(crate) unsafe fn cancel_badged_sender(tcb: *mut Tcb, call_error_info: u64) -> *mut Tcb {
    unsafe { cancel_endpoint_waiter(tcb, Some(call_error_info)).0 }
}

#[inline]
pub(crate) fn ipc_message_regs_snapshot(tcb: *const Tcb, length: u64) -> [u64; 4] {
    let mut mr_regs = [0u64; 4];
    if tcb.is_null() {
        return mr_regs;
    }
    unsafe {
        let _guard = lock_state(tcb);
        let mr_reg_n = length.min(4) as usize;
        for i in 0..mr_reg_n {
            mr_regs[i] =
                (*tcb).context.regs[crate::arch::riscv64::trap::UserRegister::A2.index() + i];
        }
        mr_regs
    }
}

pub(crate) unsafe fn write_ipc_message_regs(
    tcb: *mut Tcb,
    badge: u64,
    mr_regs: &[u64; 4],
    length: u64,
) {
    if tcb.is_null() {
        return;
    }
    unsafe {
        let _guard = lock_state(tcb);
        (*tcb).context.regs[crate::arch::riscv64::trap::UserRegister::A0.index()] = badge;
        let mr_reg_n = length.min(4) as usize;
        for i in 0..mr_reg_n {
            (*tcb).context.regs[crate::arch::riscv64::trap::UserRegister::A2.index() + i] =
                mr_regs[i];
        }
    }
}

pub(crate) unsafe fn write_fault_ipc_message_regs(
    tcb: *mut Tcb,
    badge: u64,
    info_word: u64,
    mrs: &[u64; 16],
    length: u64,
) {
    if tcb.is_null() {
        return;
    }
    unsafe {
        let _guard = lock_state(tcb);
        (*tcb).context.regs[crate::arch::riscv64::trap::UserRegister::A0.index()] = badge;
        (*tcb).context.regs[crate::arch::riscv64::trap::UserRegister::A1.index()] = info_word;
        let mr_reg_n = length.min(4).min(mrs.len() as u64) as usize;
        for i in 0..mr_reg_n {
            (*tcb).context.regs[crate::arch::riscv64::trap::UserRegister::A2.index() + i] = mrs[i];
        }
    }
}

pub(crate) unsafe fn copy_ipc_buffer_words(
    sender: *const Tcb,
    receiver: *mut Tcb,
    start_word: usize,
    count: usize,
) -> bool {
    if count == 0 {
        return true;
    }
    if sender.is_null() || receiver.is_null() {
        return false;
    }
    unsafe {
        with_tcb_pair_guard(sender, receiver, || {
            let sender_ipc_buffer = (*sender).ipc_buffer_kva;
            let receiver_ipc_buffer = (*receiver).ipc_buffer_kva;
            if sender_ipc_buffer == 0 || receiver_ipc_buffer == 0 {
                return false;
            }
            let sbuf = sender_ipc_buffer as *const u64;
            let rbuf = receiver_ipc_buffer as *mut u64;
            for i in 0..count {
                let off = start_word + i;
                *rbuf.add(off) = *sbuf.add(off);
            }
            true
        })
    }
}

pub(crate) unsafe fn write_ipc_buffer_words(
    tcb: *mut Tcb,
    start_word: usize,
    values: &[u64],
) -> bool {
    if values.is_empty() {
        return true;
    }
    if tcb.is_null() {
        return false;
    }
    unsafe {
        let _guard = lock_state(tcb);
        let ipc_buffer = (*tcb).ipc_buffer_kva;
        if ipc_buffer == 0 {
            return false;
        }
        let buf = ipc_buffer as *mut u64;
        for (i, value) in values.iter().enumerate() {
            *buf.add(start_word + i) = *value;
        }
        true
    }
}

pub(crate) unsafe fn write_ipc_buffer_word(tcb: *mut Tcb, index: usize, value: u64) -> bool {
    if tcb.is_null() {
        return false;
    }
    unsafe {
        let _guard = lock_state(tcb);
        let ipc_buffer = (*tcb).ipc_buffer_kva;
        if ipc_buffer == 0 {
            return false;
        }
        *((ipc_buffer as *mut u64).add(index)) = value;
        true
    }
}

pub(crate) unsafe fn zero_ipc_buffer_words(tcb: *mut Tcb, start_word: usize, count: usize) -> bool {
    if count == 0 {
        return true;
    }
    if tcb.is_null() {
        return false;
    }
    unsafe {
        let _guard = lock_state(tcb);
        let ipc_buffer = (*tcb).ipc_buffer_kva;
        if ipc_buffer == 0 {
            return false;
        }
        let buf = ipc_buffer as *mut u64;
        for i in 0..count {
            *buf.add(start_word + i) = 0;
        }
        true
    }
}

pub(crate) fn ipc_buffer_word_snapshot(tcb: *const Tcb, index: usize) -> u64 {
    if tcb.is_null() {
        return 0;
    }
    unsafe {
        let _guard = lock_state(tcb);
        let ipc_buffer = (*tcb).ipc_buffer_kva;
        if ipc_buffer == 0 {
            0
        } else {
            *((ipc_buffer as *const u64).add(index))
        }
    }
}

pub(crate) fn has_ipc_buffer(tcb: *const Tcb) -> bool {
    if tcb.is_null() {
        return false;
    }
    unsafe {
        let _guard = lock_state(tcb);
        (*tcb).ipc_buffer_kva != 0
    }
}

pub(crate) unsafe fn write_message_info(tcb: *mut Tcb, info_word: u64) {
    if tcb.is_null() {
        return;
    }
    unsafe {
        let _guard = lock_state(tcb);
        (*tcb).context.regs[crate::arch::riscv64::trap::UserRegister::A1.index()] = info_word;
    }
}

pub(crate) unsafe fn write_user_context(tcb: *mut Tcb, pc: Option<u64>, regs: &[(usize, u64)]) {
    if tcb.is_null() {
        return;
    }
    unsafe {
        let _guard = lock_state(tcb);
        if let Some(pc) = pc {
            (*tcb).context.pc = pc;
            (*tcb).context.restart_pc = pc;
        }
        for &(idx, value) in regs {
            if idx != 0 && idx < (*tcb).context.regs.len() {
                (*tcb).context.regs[idx] = value;
            }
        }
    }
}

#[inline]
pub(crate) fn user_context_word_snapshot(
    tcb: *const Tcb,
    context_index: usize,
    reg_index: usize,
) -> u64 {
    if tcb.is_null() {
        return 0;
    }
    unsafe {
        let _guard = lock_state(tcb);
        match context_index {
            0 => (*tcb).context.restart_pc,
            1 => (*tcb).context.regs[crate::arch::riscv64::trap::UserRegister::Ra.index()],
            _ if reg_index != 0 && reg_index < (*tcb).context.regs.len() => {
                (*tcb).context.regs[reg_index]
            }
            _ => 0,
        }
    }
}

#[inline]
pub(crate) fn sender_fault_snapshot(tcb: *const Tcb) -> (bool, u64) {
    if tcb.is_null() {
        return (false, 0);
    }
    unsafe {
        let _guard = lock_state(tcb);
        let is_fault = (*tcb).sender_is_fault != 0;
        let label = if is_fault { (*tcb).fault_label } else { 0 };
        (is_fault, label)
    }
}

#[inline]
pub(crate) fn queued_sender_snapshot(tcb: *const Tcb) -> QueuedSenderSnapshot {
    if tcb.is_null() {
        return QueuedSenderSnapshot {
            info_word: 0,
            badge: 0,
            is_call: false,
            can_grant: false,
            can_grant_reply: false,
            extra_cap_slots: [0; TCB_SENDER_EXTRA_CAPS],
            is_fault: false,
            fault_label: 0,
        };
    }
    unsafe {
        let _guard = lock_state(tcb);
        let is_fault = (*tcb).sender_is_fault != 0;
        QueuedSenderSnapshot {
            info_word: (*tcb).context.regs[crate::arch::riscv64::trap::UserRegister::A1.index()],
            badge: (*tcb).sender_badge,
            is_call: (*tcb).sender_is_call != 0,
            can_grant: (*tcb).sender_can_grant != 0,
            can_grant_reply: (*tcb).sender_can_grant_reply != 0,
            extra_cap_slots: (*tcb).sender_extra_cap_slots,
            is_fault,
            fault_label: if is_fault { (*tcb).fault_label } else { 0 },
        }
    }
}

#[inline]
pub(crate) fn fault_message_snapshot(tcb: *const Tcb) -> FaultIpcMessage {
    if tcb.is_null() {
        return FaultIpcMessage {
            label: 0,
            len: 0,
            mrs: [0; 16],
        };
    }
    unsafe {
        let _guard = lock_state(tcb);
        FaultIpcMessage {
            label: (*tcb).fault_label,
            len: (*tcb).fault_len,
            mrs: (*tcb).fault_mrs,
        }
    }
}

pub(crate) unsafe fn record_fault_message(tcb: *mut Tcb, label: u64, len: u64, mrs: [u64; 16]) {
    if tcb.is_null() {
        return;
    }
    unsafe {
        let _guard = lock_state(tcb);
        (*tcb).sender_is_fault = 1;
        (*tcb).fault_label = label;
        (*tcb).fault_len = len;
        (*tcb).fault_mrs = mrs;
    }
}

pub(crate) unsafe fn finish_reply_state(
    tcb: *mut Tcb,
    clear_fault_message: bool,
    wake: bool,
) -> u64 {
    if tcb.is_null() {
        return 0;
    }
    unsafe {
        let _guard = lock_state(tcb);
        if clear_fault_message {
            (*tcb).sender_is_fault = 0;
            (*tcb).fault_label = 0;
            (*tcb).fault_len = 0;
            (*tcb).fault_mrs = [0; 16];
        }
        (*tcb).waiting_on = 0;
        (*tcb).reply_object = 0;
        (*tcb).reply_slot = 0;
        if wake {
            (*tcb).state = ThreadState::Running as u8;
            (*tcb).sched_context
        } else {
            (*tcb).state = ThreadState::Inactive as u8;
            0
        }
    }
}

pub(crate) unsafe fn set_blocked_sender(
    tcb: *mut Tcb,
    endpoint: u64,
    is_call: bool,
    badge: u64,
    can_grant: bool,
    can_grant_reply: bool,
    extra_cap_slots: [u64; TCB_SENDER_EXTRA_CAPS],
) {
    if tcb.is_null() {
        return;
    }
    unsafe {
        let _guard = lock_state(tcb);
        clear_endpoint_ipc_state_locked(tcb);
        (*tcb).state = ThreadState::BlockedOnSend as u8;
        (*tcb).waiting_on = endpoint;
        (*tcb).sender_badge = badge;
        (*tcb).sender_can_grant = if can_grant { 1 } else { 0 };
        (*tcb).sender_can_grant_reply = if can_grant_reply { 1 } else { 0 };
        (*tcb).sender_extra_cap_slots = extra_cap_slots;
        (*tcb).sender_is_call = if is_call { 1 } else { 0 };
        (*tcb).sender_is_fault = 0;
    }
}

pub(crate) unsafe fn set_blocked_receiver(
    tcb: *mut Tcb,
    endpoint: u64,
    can_grant: bool,
    reply_cptr: u64,
    reply_object: u64,
    reply_can_grant: bool,
) {
    if tcb.is_null() {
        return;
    }
    unsafe {
        let _guard = lock_state(tcb);
        clear_endpoint_ipc_state_locked(tcb);
        (*tcb).state = ThreadState::BlockedOnReceive as u8;
        (*tcb).waiting_on = endpoint;
        (*tcb).receiver_can_grant = if can_grant { 1 } else { 0 };
        (*tcb).receive_reply_cptr = reply_cptr;
        (*tcb).receive_reply_object = reply_object;
        (*tcb).receive_reply_can_grant = if reply_can_grant { 1 } else { 0 };
    }
}

pub(crate) unsafe fn set_blocked_fault_sender(
    tcb: *mut Tcb,
    endpoint: u64,
    badge: u64,
    can_grant: bool,
    can_grant_reply: bool,
    label: u64,
    len: u64,
    mrs: [u64; 16],
) {
    if tcb.is_null() {
        return;
    }
    unsafe {
        let _guard = lock_state(tcb);
        clear_endpoint_ipc_state_locked(tcb);
        (*tcb).state = ThreadState::BlockedOnSend as u8;
        (*tcb).waiting_on = endpoint;
        (*tcb).sender_badge = badge;
        (*tcb).sender_can_grant = if can_grant { 1 } else { 0 };
        (*tcb).sender_can_grant_reply = if can_grant_reply { 1 } else { 0 };
        (*tcb).sender_extra_cap_slots = [0; TCB_SENDER_EXTRA_CAPS];
        (*tcb).sender_is_call = 1;
        (*tcb).sender_is_fault = 1;
        (*tcb).fault_label = label;
        (*tcb).fault_len = len;
        (*tcb).fault_mrs = mrs;
    }
}

pub(crate) unsafe fn start_receiver_rendezvous(tcb: *mut Tcb) -> (u64, u64, bool) {
    if tcb.is_null() {
        return (0, 0, false);
    }
    unsafe {
        let _guard = lock_state(tcb);
        (*tcb).waiting_on = 0;
        (
            (*tcb).receive_reply_cptr,
            (*tcb).receive_reply_object,
            (*tcb).receive_reply_can_grant != 0,
        )
    }
}

pub(crate) unsafe fn wake_blocked_receiver_after_send(tcb: *mut Tcb) {
    if tcb.is_null() {
        return;
    }
    unsafe {
        crate::object::reply::unbind_receiver(tcb);
    }
    unsafe {
        let _guard = lock_state(tcb);
        (*tcb).waiting_on = 0;
        clear_endpoint_ipc_state_locked(tcb);
        (*tcb).state = ThreadState::Running as u8;
    }
}

pub(crate) unsafe fn finish_receiver_rendezvous(tcb: *mut Tcb) {
    if tcb.is_null() {
        return;
    }
    unsafe {
        let _guard = lock_state(tcb);
        clear_endpoint_ipc_state_locked(tcb);
        (*tcb).state = ThreadState::Running as u8;
    }
}

pub(crate) unsafe fn deactivate_queued_call_sender(tcb: *mut Tcb) {
    if tcb.is_null() {
        return;
    }
    unsafe {
        let _guard = lock_state(tcb);
        (*tcb).waiting_on = 0;
        (*tcb).context.pc = (*tcb).context.pc.wrapping_sub(4);
        (*tcb).context.restart_pc = (*tcb).context.pc;
        (*tcb).state = ThreadState::Inactive as u8;
        (*tcb).sender_is_call = 0;
    }
}

pub(crate) unsafe fn wake_queued_sender(tcb: *mut Tcb) {
    if tcb.is_null() {
        return;
    }
    unsafe {
        let _guard = lock_state(tcb);
        (*tcb).waiting_on = 0;
        (*tcb).state = ThreadState::Running as u8;
    }
}

pub(crate) unsafe fn finish_call_sender_after_rendezvous(tcb: *mut Tcb, blocked_on_reply: bool) {
    if tcb.is_null() {
        return;
    }
    unsafe {
        let _guard = lock_state(tcb);
        if blocked_on_reply {
            (*tcb).state = ThreadState::BlockedOnReply as u8;
        } else {
            (*tcb).context.pc = (*tcb).context.pc.wrapping_sub(4);
            (*tcb).context.restart_pc = (*tcb).context.pc;
            (*tcb).state = ThreadState::Inactive as u8;
        }
        (*tcb).waiting_on = 0;
        (*tcb).sender_is_fault = 0;
    }
}

pub(crate) unsafe fn clear_bound_notification_if(tcb: *mut Tcb, notification: u64) -> bool {
    if tcb.is_null() {
        return false;
    }
    unsafe {
        let _guard = lock_state(tcb);
        if (*tcb).bound_notification != notification {
            return false;
        }
        (*tcb).bound_notification = 0;
        true
    }
}

#[inline]
pub(crate) fn bound_notification_snapshot(tcb: *const Tcb) -> u64 {
    if tcb.is_null() {
        return 0;
    }
    unsafe {
        let _guard = lock_state(tcb);
        (*tcb).bound_notification
    }
}

pub unsafe fn set_ipc_buffer(tcb: *mut Tcb, buffer_uva: u64, buffer_cap: Cap) {
    if tcb.is_null() {
        return;
    }
    unsafe {
        let _guard = lock_state(tcb);
        if buffer_uva == 0 {
            (*tcb).ipc_buffer_uva = 0;
            (*tcb).ipc_buffer_kva = 0;
        } else {
            (*tcb).ipc_buffer_uva = buffer_uva;
            (*tcb).ipc_buffer_kva = buffer_cap.frame_base_ptr();
        }
    }
}

#[inline]
pub(crate) unsafe fn cap_slot(tcb: *mut Tcb, index: usize) -> *mut Cte {
    if tcb.is_null() || index >= TCB_CNODE_ENTRIES {
        return core::ptr::null_mut();
    }
    crate::kernel::smp::debug_assert_kernel_lock_held();
    unsafe { &raw mut (*tcb).ctes[index] }
}

#[inline]
pub(crate) unsafe fn cap_slot_base(tcb: *mut Tcb) -> *mut Cte {
    unsafe { cap_slot(tcb, 0) }
}

#[inline]
unsafe fn cap_slot_snapshot_locked(tcb: *const Tcb, index: usize) -> Cap {
    if index >= TCB_CNODE_ENTRIES {
        return Cap::null();
    }
    unsafe { (*tcb).ctes[index].cap }
}

#[inline]
unsafe fn ready_queue_contains(q: &Queue, tcb: *mut Tcb) -> bool {
    let mut cur = q.head;
    while !cur.is_null() {
        if cur == tcb {
            return true;
        }
        cur = unsafe { (*cur).queue_next as *mut Tcb };
    }
    false
}

unsafe fn can_enqueue_ready(q: &Queue, tcb: *mut Tcb) -> bool {
    unsafe { !ready_queue_contains(q, tcb) && (*tcb).queue_next == 0 && (*tcb).queue_prev == 0 }
}

unsafe fn enqueue_tail_locked(q: &mut Queue, tcb: *mut Tcb) -> bool {
    if tcb.is_null() {
        return false;
    }
    let tcb_u = tcb as u64;
    unsafe {
        if !can_enqueue_ready(q, tcb) {
            return false;
        }
        (*tcb).queue_next = 0;
        (*tcb).queue_prev = q.tail as u64;
        if q.tail.is_null() {
            q.head = tcb;
        } else {
            (*(q.tail)).queue_next = tcb_u;
        }
        q.tail = tcb;
    }
    true
}

/// Add `tcb` to the head of its priority's queue. No-op if already
/// linked (i.e. `queue_next` or `queue_prev` non-zero, or `head == tcb`).
pub unsafe fn enqueue(tcb: *mut Tcb) {
    if tcb.is_null() {
        return;
    }
    let snapshot = runqueue_snapshot(tcb);
    let has_budget = snapshot.sched_context != 0
        && unsafe { crate::object::sched_context::has_budget(snapshot.sched_context) };
    if (snapshot.state != ThreadState::Running as u8
        && snapshot.state != ThreadState::Restart as u8)
        || snapshot.sched_context == 0
        || !has_budget
    {
        return;
    }
    let enqueued = RUNQUEUES[snapshot.core]
        .with_mut(|runqueue| unsafe { runqueue.enqueue_head(snapshot.prio, tcb) });
    if enqueued {
        crate::kernel::smp::wake_core(snapshot.core);
    }
}

unsafe fn enqueue_head_locked(q: &mut Queue, tcb: *mut Tcb) -> bool {
    let tcb_u = tcb as u64;
    unsafe {
        if !can_enqueue_ready(q, tcb) {
            return false;
        }
        (*tcb).queue_next = q.head as u64;
        (*tcb).queue_prev = 0;
        if q.head.is_null() {
            q.tail = tcb;
        } else {
            (*(q.head)).queue_prev = tcb_u;
        }
        q.head = tcb;
    }
    true
}

/// Unlink `tcb` from its priority's queue. No-op if not currently linked.
pub unsafe fn dequeue(tcb: *mut Tcb) {
    if tcb.is_null() {
        return;
    }
    let prio_hint = priority_snapshot(tcb);
    let mut core = 0;
    while core < MAX_NUM_NODES {
        if unsafe { dequeue_from_priority(core, prio_hint, tcb) } {
            return;
        }
        core += 1;
    }

    let mut prio = 0;
    while prio < NUM_PRIORITIES {
        if prio != prio_hint {
            let mut core = 0;
            while core < MAX_NUM_NODES {
                if unsafe { dequeue_from_priority(core, prio, tcb) } {
                    return;
                }
                core += 1;
            }
        }
        prio += 1;
    }
}

unsafe fn dequeue_from_priority(core: usize, prio: usize, tcb: *mut Tcb) -> bool {
    RUNQUEUES[core].with_mut(|runqueue| unsafe { runqueue.dequeue_from_priority(prio, tcb) })
}

unsafe fn unlink_from_ready_queue(q: &mut Queue, tcb: *mut Tcb) -> bool {
    unsafe {
        let prev = (*tcb).queue_prev as *mut Tcb;
        let next = (*tcb).queue_next as *mut Tcb;
        if !prev.is_null() {
            (*prev).queue_next = next as u64;
        } else {
            q.head = next;
        }
        if !next.is_null() {
            (*next).queue_prev = prev as u64;
        } else {
            q.tail = prev;
        }
        (*tcb).queue_next = 0;
        (*tcb).queue_prev = 0;

        q.head.is_null()
    }
}

/// Move `tcb` to the tail of its own priority's queue. Used by
/// `seL4_Yield` to surrender the CPU to a same-priority peer.
pub unsafe fn rotate_to_tail(tcb: *mut Tcb) {
    if tcb.is_null() {
        return;
    }
    let snapshot = runqueue_snapshot(tcb);
    let has_budget = snapshot.sched_context != 0
        && unsafe { crate::object::sched_context::has_budget(snapshot.sched_context) };
    RUNQUEUES[snapshot.core].with_mut(|runqueue| unsafe {
        let queue = runqueue.queue(snapshot.prio);
        if has_budget && queue.head == tcb && queue.tail == tcb {
            return; // singleton, nothing to do
        }
        let _ = runqueue.dequeue_from_priority(snapshot.prio, tcb);
        if has_budget {
            let _ = runqueue.enqueue_tail(snapshot.prio, tcb);
        }
    });
}

/// Pick the highest-priority ready TCB, or `null` if all queues empty.
///
/// O(1) on the 256 priorities: scan the 4-word ready bitmap from MSB
/// down, then `head` of the first bin we find.
pub fn schedule() -> *mut Tcb {
    let candidate = peek_schedule();
    if !candidate.is_null() {
        unsafe {
            dequeue(candidate);
        }
    }
    candidate
}

/// Return the highest-priority ready TCB without consuming it from the ready
/// queue. Stale non-runnable queue entries are still discarded.
pub fn peek_schedule() -> *mut Tcb {
    loop {
        let candidate = schedule_head();
        if candidate.is_null() {
            return null_mut();
        }
        if unsafe { is_runnable_on_current_core(candidate) } {
            return candidate;
        }
        unsafe {
            dequeue(candidate);
        }
    }
}

fn schedule_head() -> *mut Tcb {
    let core = crate::kernel::smp::current_core_id();
    RUNQUEUES[core].with_mut(|runqueue| {
        for word_idx in (0..4).rev() {
            let mut bits = runqueue.ready_bits(word_idx);
            while bits != 0 {
                // Highest set bit in `bits` ⇒ highest priority in this word.
                let bit = 63 - bits.leading_zeros() as usize;
                let prio = word_idx * 64 + bit;
                let head = runqueue.queue_mut(prio).head;
                if !head.is_null() {
                    return head;
                }

                let stale_bit = 1u64 << bit;
                runqueue.clear_ready_bit(prio);
                bits &= !stale_bit;
            }
        }
        null_mut()
    })
}

#[inline]
pub unsafe fn is_runnable_on_current_core(tcb: *const Tcb) -> bool {
    if tcb.is_null() {
        return false;
    }
    let (state, sched_context, affinity) = unsafe {
        let _guard = lock_state(tcb);
        ((*tcb).state, (*tcb).sched_context, (*tcb).affinity)
    };
    (state == ThreadState::Running as u8 || state == ThreadState::Restart as u8)
        && sched_context != 0
        && core_for_affinity(affinity) == crate::kernel::smp::current_core_id()
        && unsafe { crate::object::sched_context::has_budget(sched_context) }
}

#[inline]
unsafe fn sched_snapshot(tcb: *const Tcb) -> (u8, u64, u8) {
    unsafe {
        let _guard = lock_state(tcb);
        ((*tcb).state, (*tcb).sched_context, (*tcb).affinity)
    }
}

/// If the local hart trapped while running a TCB whose affinity was moved to
/// another core, publish it on that target core's runqueue before this hart
/// schedules something else.
pub unsafe fn enqueue_if_migrated_from_current_core(tcb: *mut Tcb) {
    if tcb.is_null() {
        return;
    }
    let (state, sched_context, affinity) = unsafe { sched_snapshot(tcb) };
    if (state == ThreadState::Running as u8 || state == ThreadState::Restart as u8)
        && sched_context != 0
        && core_for_affinity(affinity) != crate::kernel::smp::current_core_id()
        && unsafe { crate::object::sched_context::has_budget(sched_context) }
    {
        unsafe { enqueue(tcb) };
    }
}

pub const TCB_NAME_LEN: usize = 32;

/// Mirrors `_thread_state` in `kernel/include/object/structures.h`. The
/// numbering doesn't need to match upstream because we never expose this
/// over the ABI; we only use it for our own scheduler.
#[repr(u8)]
#[derive(Copy, Clone, PartialEq, Eq, Debug)]
pub enum ThreadState {
    Inactive = 0,
    Restart = 1,
    Running = 2,
    BlockedOnReceive = 3,
    BlockedOnSend = 4,
    BlockedOnReply = 5,
    BlockedOnNotification = 6,
    Idle = 7,
}

#[repr(C, align(32))]
pub struct Tcb {
    /// seL4-style CTEs embedded in the TCB object. RISC-V MCS uses slots
    /// 0..4 for CSpace, VSpace, IPC buffer, fault handler, and timeout
    /// handler; the remaining slots exist because ZombieTCB uses radix 4.
    pub ctes: [Cte; TCB_CNODE_ENTRIES],

    /// Saved user-mode register state. The trap path restores this on
    /// `sret` once a scheduler picks the TCB.
    pub context: UserContext,

    /// Scheduling.
    pub state: u8,
    pub priority: u8,
    pub mcp: u8,
    pub domain: u8,
    pub affinity: u8,
    pub time_slice_ticks: u8,
    pub _sched_pad: [u8; 2],

    /// User-mode VA at which the IPC buffer is mapped.
    pub ipc_buffer_uva: u64,
    /// Kernel-window VA reachable via the PSpace mapping of the IPC
    /// buffer frame — i.e. what `Thread.ipc_buffer_kva` would point at
    /// after `restore_user_context` swaps to this thread. Lazily
    /// resolved; 0 means "not yet set up".
    pub ipc_buffer_kva: u64,

    /// MCS scheduling context currently bound to this TCB, or 0 for passive.
    pub sched_context: u64,
    /// MCS `SchedContext_YieldTo` bookkeeping. `yield_to_sc` is set on
    /// the yielding TCB; `yield_from_tcb` is set on the target TCB.
    pub yield_to_sc: u64,
    pub yield_to_consumed_start: u64,
    pub yield_from_tcb: u64,

    /// Pointer (PSpace KVA) to the bound `Notification`, or 0.
    pub bound_notification: u64,

    /// Ready-queue links. Both are PSpace KVAs of other `Tcb`s, or 0.
    pub queue_next: u64,
    pub queue_prev: u64,

    /// Object the TCB is currently blocked on (Endpoint / Notification
    /// KVA), if any. Used to dequeue on cancel / destroy.
    pub waiting_on: u64,
    pub receiver_can_grant: u8,
    pub receive_reply_cptr: u64,
    pub receive_reply_object: u64,
    pub receive_reply_can_grant: u8,

    /// Slot holding the MCS reply cap that parked this TCB on a reply object.
    pub reply_slot: u64,
    pub reply_object: u64,

    /// Badge from the cap used to Send / Call. Stashed when a sender
    /// blocks on an Endpoint so the eventual receiver can read it back
    /// without re-walking the sender's CSpace.
    pub sender_badge: u64,
    pub sender_extra_cap_slots: [u64; TCB_SENDER_EXTRA_CAPS],
    pub sender_can_grant: u8,
    pub sender_can_grant_reply: u8,
    /// `1` iff the queued-up Send was originally a `seL4_Call`. The
    /// receiver consults this on rendezvous to decide whether to put
    /// the sender into `BlockedOnReply` (Call) or wake it directly
    /// (plain Send).
    pub sender_is_call: u8,

    /// `1` iff the queued-up Call is a synthetic fault IPC. Fault IPC
    /// must not borrow the faulting thread's message registers: the
    /// handler reply restarts the trapped instruction, so the original
    /// user register file has to survive intact.
    pub sender_is_fault: u8,
    pub fault_label: u64,
    pub fault_len: u64,
    pub fault_mrs: [u64; 16],

    /// Debug name, populated by `seL4_DebugNameThread`. NUL-padded.
    pub name: [u8; TCB_NAME_LEN],
}

// Compile-time sanity: must fit inside the 2 KiB Untyped slab (= 2048
// bytes for SEL4_TCB_BITS = 11), including the embedded TCB CTEs.
const _: () = {
    assert!(core::mem::size_of::<Tcb>() <= 2048);
};

impl Tcb {
    /// All-zero TCB constructor for static / BSS use (the rootserver TCB
    /// is created this way; user-allocated TCBs go through `init` after
    /// `Untyped_Retype` zeroes their slab).
    pub const fn zero() -> Self {
        Tcb {
            ctes: [Cte::null(); TCB_CNODE_ENTRIES],
            context: UserContext {
                regs: [0; 32],
                pc: 0,
                sstatus: 0,
                restart_pc: 0,
            },
            state: 0,
            priority: 0,
            mcp: 0,
            domain: 0,
            affinity: 0,
            time_slice_ticks: 0,
            _sched_pad: [0; 2],
            ipc_buffer_uva: 0,
            ipc_buffer_kva: 0,
            sched_context: 0,
            yield_to_sc: 0,
            yield_to_consumed_start: 0,
            yield_from_tcb: 0,
            bound_notification: 0,
            queue_next: 0,
            queue_prev: 0,
            waiting_on: 0,
            receiver_can_grant: 0,
            receive_reply_cptr: 0,
            receive_reply_object: 0,
            receive_reply_can_grant: 0,
            reply_slot: 0,
            reply_object: 0,
            sender_badge: 0,
            sender_extra_cap_slots: [0; TCB_SENDER_EXTRA_CAPS],
            sender_can_grant: 0,
            sender_can_grant_reply: 0,
            sender_is_call: 0,
            sender_is_fault: 0,
            fault_label: 0,
            fault_len: 0,
            fault_mrs: [0; 16],
            name: [0; TCB_NAME_LEN],
        }
    }
}

/// Reborrow a Thread cap as a `*mut Tcb`. Returns `null_mut()` for a
/// null cap. Caller guarantees the underlying memory has not been
/// recycled (`finalize` clears caps before the Untyped is reset, so as
/// long as you go through the live cap you're safe).
#[inline]
pub fn from_cap(cap: Cap) -> *mut Tcb {
    let p = cap.thread_ptr();
    if p == 0 { null_mut() } else { p as *mut Tcb }
}

/// Initialise a freshly-retyped 2 KiB TCB slab.
///
/// `Untyped_Retype` already zeroed the memory; we only stamp the bits
/// where 0 isn't the right resting value (currently just sstatus so a
/// future `restore_user_context` returns to U-mode with interrupts enabled).
pub unsafe fn init(tcb_kva: u64) {
    let t = tcb_kva as *mut Tcb;
    // sstatus.SPIE = 1 -> sret re-enables interrupts in U-mode.
    // sstatus.SPP  = 0 -> sret enters U-mode (already 0).
    unsafe {
        let _guard = lock_state(t);
        (*t).state = ThreadState::Inactive as u8;
        (*t).time_slice_ticks = DEFAULT_TIME_SLICE_TICKS;
        (*t).context.sstatus = crate::arch::riscv64::trap::USER_SSTATUS;
    }
}

/// Detach `tcb` from any Endpoint or Notification wait list it might be
/// queued on (because of a prior blocking Send / Recv / Call / Wait).
/// Safe to call on a TCB that isn't waiting on anything — it
/// short-circuits on `waiting_on == 0`.
///
/// Dispatch on `tcb.state` so we route to the right object:
/// BlockedOn{Receive,Send} → Endpoint; BlockedOnNotification → Notification.
/// BlockedOnReply has no endpoint wait object; `suspend` handles the reply
/// stack through `reply::remove_tcb`.
unsafe fn unlink_from_wait_object(tcb: *mut Tcb) {
    if tcb.is_null() {
        return;
    }
    let (obj, st) = unsafe {
        let _guard = lock_state(tcb);
        ((*tcb).waiting_on, (*tcb).state)
    };
    if obj == 0 {
        return;
    }
    unsafe {
        if st == ThreadState::BlockedOnReceive as u8 {
            crate::object::reply::unbind_receiver(tcb);
        }
        if st == ThreadState::BlockedOnNotification as u8 {
            let ntfn = obj as *mut crate::object::notification::Notification;
            crate::object::notification::remove_waiter(ntfn, tcb);
        } else {
            let ep = obj as *mut crate::object::endpoint::Endpoint;
            crate::object::endpoint::remove_waiter(ep, tcb);
        }
        let _guard = lock_state(tcb);
        (*tcb).waiting_on = 0;
        clear_endpoint_ipc_state_locked(tcb);
    }
}

/// Per-invocation primitives. They mark the TCB runnable / non-runnable
/// and update the per-core ready queue accordingly. The actual CPU swap
/// happens at the next `kernel_exit()` boundary (top of `restore_user_context`).
pub unsafe fn suspend(tcb: *mut Tcb) {
    if tcb.is_null() {
        return;
    }
    unsafe {
        // A suspended TCB must leave any EP wait list it's queued on,
        // otherwise the EP would later try to `pop_head` a TCB whose
        // backing slab might be reused.
        unlink_from_wait_object(tcb);
        dequeue(tcb);
        crate::object::sched_context::complete_yield_to_target(tcb);
        crate::object::sched_context::cancel_yield_to(tcb);
        crate::object::reply::remove_tcb(tcb);
        let _guard = lock_state(tcb);
        (*tcb).state = ThreadState::Inactive as u8;
        crate::kernel::smp::wake_current_core_of_tcb(tcb);
    }
}

pub unsafe fn resume(tcb: *mut Tcb) {
    if tcb.is_null() {
        return;
    }
    let state = unsafe {
        let _guard = lock_state(tcb);
        (*tcb).state
    };
    let stopped = state == ThreadState::Inactive as u8
        || state == ThreadState::BlockedOnReceive as u8
        || state == ThreadState::BlockedOnSend as u8
        || state == ThreadState::BlockedOnNotification as u8
        || state == ThreadState::BlockedOnReply as u8;
    if !stopped {
        return;
    }
    unsafe {
        unlink_from_wait_object(tcb);
        crate::object::reply::remove_tcb(tcb);
        {
            let _guard = lock_state(tcb);
            if (*tcb).time_slice_ticks == 0 {
                (*tcb).time_slice_ticks = DEFAULT_TIME_SLICE_TICKS;
            }
            (*tcb).state = ThreadState::Restart as u8;
        }
        enqueue(tcb);
    }
}

pub unsafe fn set_priority(tcb: *mut Tcb, prio: u8) {
    if tcb.is_null() {
        return;
    }
    unsafe {
        dequeue(tcb);
        {
            let _guard = lock_state(tcb);
            (*tcb).priority = prio;
        }
        let (state, waiting_on) = {
            let _guard = lock_state(tcb);
            ((*tcb).state, (*tcb).waiting_on)
        };
        if waiting_on != 0
            && (state == ThreadState::BlockedOnReceive as u8
                || state == ThreadState::BlockedOnSend as u8)
        {
            crate::object::endpoint::reorder_waiter(
                waiting_on as *mut crate::object::endpoint::Endpoint,
                tcb,
            );
        } else if waiting_on != 0 && state == ThreadState::BlockedOnNotification as u8 {
            crate::object::notification::reorder_waiter(
                waiting_on as *mut crate::object::notification::Notification,
                tcb,
            );
        } else if state == ThreadState::Running as u8 || state == ThreadState::Restart as u8 {
            enqueue(tcb);
        }
    }
}

pub unsafe fn set_mcp(tcb: *mut Tcb, mcp: u8) {
    if tcb.is_null() {
        return;
    }
    unsafe {
        let _guard = lock_state(tcb);
        (*tcb).mcp = mcp;
    }
}

pub unsafe fn set_domain(tcb: *mut Tcb, domain: u8) {
    if tcb.is_null() {
        return;
    }
    unsafe {
        let _guard = lock_state(tcb);
        (*tcb).domain = domain;
    }
}

pub unsafe fn set_affinity(tcb: *mut Tcb, affinity: u8) {
    if tcb.is_null() {
        return;
    }
    unsafe {
        let state = {
            let _guard = lock_state(tcb);
            (*tcb).state
        };
        let was_runnable =
            state == ThreadState::Running as u8 || state == ThreadState::Restart as u8;
        let running_core = crate::kernel::smp::current_core_of_tcb(tcb);
        let migrate_current = running_core.is_some_and(|core| core != affinity as usize);
        if was_runnable {
            dequeue(tcb);
        }
        {
            let _guard = lock_state(tcb);
            (*tcb).affinity = affinity;
        }
        if was_runnable && !migrate_current {
            enqueue(tcb);
        }
        if migrate_current {
            crate::kernel::smp::wake_current_core_of_tcb(tcb);
        }
    }
}

pub unsafe fn set_tls_base(tcb: *mut Tcb, tls_base: u64) {
    if tcb.is_null() {
        return;
    }
    unsafe {
        let _guard = lock_state(tcb);
        (*tcb).context.regs[crate::arch::riscv64::trap::UserRegister::Tp.index()] = tls_base;
    }
}

pub unsafe fn set_debug_name(tcb: *mut Tcb, name: *const u8, len: usize) {
    if tcb.is_null() {
        return;
    }
    unsafe {
        let _guard = lock_state(tcb);
        let dst = &mut (*tcb).name;
        dst.fill(0);
        let copy_len = core::cmp::min(len, TCB_NAME_LEN.saturating_sub(1));
        core::ptr::copy_nonoverlapping(name, dst.as_mut_ptr(), copy_len);
    }
}

pub unsafe fn bind_notification(tcb: *mut Tcb, ntfn_kva: u64) {
    if tcb.is_null() || ntfn_kva == 0 {
        return;
    }
    let ntfn_ptr = ntfn_kva as *mut crate::object::notification::Notification;
    loop {
        // Snapshot first, then take wait-object and TCB marker guards under
        // the seL4-style BKL.
        let prev_ntfn = unsafe {
            let _guard = lock_state(tcb);
            (*tcb).bound_notification
        };
        let done = unsafe {
            if prev_ntfn != 0 {
                let prev = prev_ntfn as *mut crate::object::notification::Notification;
                let _wait_guard =
                    crate::object::wait_queue_lock::lock_pair(prev.cast(), ntfn_ptr.cast());
                let _tcb_guard = lock_state(tcb);
                if (*tcb).bound_notification != prev_ntfn {
                    false
                } else {
                    (*prev).set_bound_tcb(0);
                    (*tcb).bound_notification = ntfn_kva;
                    (*ntfn_ptr).set_bound_tcb(tcb as u64);
                    true
                }
            } else {
                let _wait_guard = crate::object::notification::lock_queue(ntfn_ptr);
                let _tcb_guard = lock_state(tcb);
                if (*tcb).bound_notification != 0 {
                    false
                } else {
                    (*tcb).bound_notification = ntfn_kva;
                    (*ntfn_ptr).set_bound_tcb(tcb as u64);
                    true
                }
            }
        };
        if done {
            return;
        }
    }
}

pub unsafe fn unbind_notification(tcb: *mut Tcb) {
    if tcb.is_null() {
        return;
    }
    loop {
        let ntfn_kva = unsafe {
            let _guard = lock_state(tcb);
            (*tcb).bound_notification
        };
        if ntfn_kva == 0 {
            return;
        }
        let done = unsafe {
            let p = ntfn_kva as *mut crate::object::notification::Notification;
            let _wait_guard = crate::object::notification::lock_queue(p);
            let _tcb_guard = lock_state(tcb);
            if (*tcb).bound_notification != ntfn_kva {
                false
            } else {
                (*p).set_bound_tcb(0);
                (*tcb).bound_notification = 0;
                true
            }
        };
        if done {
            return;
        }
    }
}

/// Wipe a TCB on destruction. Called from `finalize_cap(Thread)`.
///
/// Drop the bound notification, sched-context binding, IPC wait state, and
/// runnable state so a stale pointer to this memory (post-Retype) cannot
/// remain live during the in-flight Revoke.
pub unsafe fn finalize(tcb: *mut Tcb) {
    crate::kernel::smp::debug_assert_kernel_lock_held();
    if tcb.is_null() {
        return;
    }
    unsafe {
        // Match seL4 finaliseCap(Thread): unbind the notification first,
        // detach the bound sched context and complete any yield-to yielder,
        // then run the normal suspend path before clearing Rust-local state.
        unbind_notification(tcb);
        let sched_context = {
            let _guard = lock_state(tcb);
            (*tcb).sched_context
        };
        if sched_context != 0 {
            let _ = crate::object::sched_context::try_unbind_tcb(sched_context, tcb);
            crate::object::sched_context::complete_yield_to_yielder(sched_context);
        }
        suspend(tcb);
        clear_finalized_state(tcb);
    }
}

unsafe fn clear_finalized_state(tcb: *mut Tcb) {
    unsafe {
        let _guard = lock_state(tcb);
        (*tcb).queue_next = 0;
        (*tcb).queue_prev = 0;
        (*tcb).waiting_on = 0;
        (*tcb).reply_slot = 0;
        (*tcb).reply_object = 0;
        clear_endpoint_ipc_state_locked(tcb);
        (*tcb).sched_context = 0;
        (*tcb).yield_to_sc = 0;
        (*tcb).yield_to_consumed_start = 0;
        (*tcb).yield_from_tcb = 0;
        (*tcb).bound_notification = 0;
        (*tcb).ipc_buffer_uva = 0;
        (*tcb).ipc_buffer_kva = 0;
        (*tcb).state = ThreadState::Inactive as u8;
    }
}
