//! Minimal MCS scheduling-context object state.
//!
//! This is intentionally a conformance scaffold: it records bindings and
//! configuration so the MCS ABI can create, configure, bind, unbind, and
//! pass scheduling contexts through the existing per-core scheduler.

#![allow(dead_code)]

use crate::kernel::smp::{BklCell, BklObjectGuard};
use crate::object::tcb::{self, Tcb};

const MAX_TRACKED_SCHED_CONTEXTS: usize = 256;
const MIN_BUDGET_TICKS: u64 =
    crate::abi::constants::SEL4_MIN_BUDGET_US * crate::abi::constants::SEL4_TIMER_TICKS_PER_US;
const TIMER_PRECISION_TICKS: u64 = crate::abi::constants::SEL4_TIMER_TICKS_PER_US;
const ROUND_ROBIN_TIMER_PRECISION_TICKS: u64 =
    crate::abi::constants::SEL4_TIMER_TICKS_PER_US * 5000;
const LONG_ROUND_ROBIN_BUDGET_THRESHOLD_TICKS: u64 =
    crate::abi::constants::SEL4_TIMER_TICKS_PER_US * 50_000;
const REFILL_ARRAY_OFFSET_BYTES: usize = crate::abi::constants::SEL4_CORE_SCHED_CONTEXT_BYTES
    as usize
    - crate::abi::constants::SEL4_MIN_REFILLS as usize
        * crate::abi::constants::SEL4_REFILL_SIZE_BYTES as usize;

struct TrackedSchedContexts {
    entries: [u64; MAX_TRACKED_SCHED_CONTEXTS],
    count: usize,
}

impl TrackedSchedContexts {
    const fn new() -> Self {
        Self {
            entries: [0; MAX_TRACKED_SCHED_CONTEXTS],
            count: 0,
        }
    }

    fn register(&mut self, sc_kva: u64) {
        let mut i = 0;
        while i < self.count {
            if self.entries[i] == sc_kva {
                return;
            }
            i += 1;
        }
        if self.count < MAX_TRACKED_SCHED_CONTEXTS {
            self.entries[self.count] = sc_kva;
            self.count += 1;
        }
    }

    fn unregister(&mut self, sc_kva: u64) {
        let mut i = 0;
        while i < self.count {
            if self.entries[i] == sc_kva {
                self.count -= 1;
                self.entries[i] = self.entries[self.count];
                self.entries[self.count] = 0;
                return;
            }
            i += 1;
        }
    }

    fn snapshot(&self) -> ([u64; MAX_TRACKED_SCHED_CONTEXTS], usize) {
        let mut snapshot = [0; MAX_TRACKED_SCHED_CONTEXTS];
        let mut i = 0;
        while i < self.count {
            snapshot[i] = self.entries[i];
            i += 1;
        }
        (snapshot, self.count)
    }
}

static TRACKED_SCHED_CONTEXTS: BklCell<TrackedSchedContexts> =
    BklCell::new(TrackedSchedContexts::new());
static RELEASE_SCHED_CONTEXTS: BklCell<TrackedSchedContexts> =
    BklCell::new(TrackedSchedContexts::new());

pub(crate) type SchedContextLockGuard = BklObjectGuard;

#[repr(C)]
#[derive(Copy, Clone)]
pub struct Refill {
    pub time: u64,
    pub amount: u64,
}

const _: () = {
    assert!(
        core::mem::size_of::<Refill>() == crate::abi::constants::SEL4_REFILL_SIZE_BYTES as usize
    );
    assert!(core::mem::align_of::<Refill>() >= 8);
};

#[repr(C)]
pub struct SchedContext {
    pub period: u64,
    pub consumed: u64,
    pub core: u64,
    pub tcb: u64,
    pub reply: u64,
    pub notification: u64,
    pub badge: u64,
    pub yield_from: u64,
    pub refill_max: u64,
    pub refill_head: u64,
    pub refill_tail: u64,
    pub sporadic: u64,
    pub refills: [Refill; 2],
}

const _: () = {
    assert!(
        core::mem::size_of::<SchedContext>()
            == crate::abi::constants::SEL4_CORE_SCHED_CONTEXT_BYTES as usize
    );
    assert!(core::mem::align_of::<SchedContext>() >= 8);
};

#[inline]
pub(crate) fn lock(_sc_kva: u64) -> SchedContextLockGuard {
    BklObjectGuard::new()
}

#[inline]
unsafe fn tcb_sched_context(tcb: *mut Tcb) -> u64 {
    tcb::sched_context_snapshot(tcb)
}

pub unsafe fn init(sc_kva: u64, core: u64) {
    crate::kernel::smp::debug_assert_kernel_lock_held();
    let sc = sc_kva as *mut SchedContext;
    unsafe {
        core::ptr::write_bytes(sc as *mut u8, 0, core::mem::size_of::<SchedContext>());
        (*sc).core = core;
    }
}

fn register(sc_kva: u64) {
    TRACKED_SCHED_CONTEXTS.with_mut(|tracked| tracked.register(sc_kva));
}

fn unregister(sc_kva: u64) {
    TRACKED_SCHED_CONTEXTS.with_mut(|tracked| tracked.unregister(sc_kva));
    RELEASE_SCHED_CONTEXTS.with_mut(|tracked| tracked.unregister(sc_kva));
}

fn register_release(sc_kva: u64) {
    RELEASE_SCHED_CONTEXTS.with_mut(|tracked| tracked.register(sc_kva));
}

fn unregister_release(sc_kva: u64) {
    RELEASE_SCHED_CONTEXTS.with_mut(|tracked| tracked.unregister(sc_kva));
}

fn release_sched_context_snapshot() -> ([u64; MAX_TRACKED_SCHED_CONTEXTS], usize) {
    RELEASE_SCHED_CONTEXTS.with_ref(TrackedSchedContexts::snapshot)
}

pub unsafe fn configure(
    sc_kva: u64,
    budget: u64,
    period: u64,
    extra_refills: u64,
    badge: u64,
    flags: u64,
) {
    configure_inner(sc_kva, None, budget, period, extra_refills, badge, flags);
}

pub unsafe fn configure_on_core(
    sc_kva: u64,
    core: u64,
    budget: u64,
    period: u64,
    extra_refills: u64,
    badge: u64,
    flags: u64,
) {
    configure_inner(
        sc_kva,
        Some(core),
        budget,
        period,
        extra_refills,
        badge,
        flags,
    );
}

fn configure_inner(
    sc_kva: u64,
    core: Option<u64>,
    budget: u64,
    period: u64,
    extra_refills: u64,
    badge: u64,
    flags: u64,
) {
    let sc = sc_kva as *mut SchedContext;
    let budget_ticks = budget.saturating_mul(crate::abi::constants::SEL4_TIMER_TICKS_PER_US);
    let period_ticks = period.saturating_mul(crate::abi::constants::SEL4_TIMER_TICKS_PER_US);
    let affinity_update = unsafe {
        register(sc_kva);
        let _guard = lock(sc_kva);
        if let Some(core) = core {
            (*sc).core = core;
        }
        (*sc).period = if budget == period { 0 } else { period_ticks };
        (*sc).badge = badge;
        let max_refills = if budget == period {
            crate::abi::constants::SEL4_MIN_REFILLS
        } else {
            crate::abi::constants::SEL4_MIN_REFILLS + extra_refills
        };
        (*sc).sporadic = flags & 0x1;
        let now = now_ticks();
        if (*sc).period == 0 {
            refill_new(sc, max_refills, now, budget_ticks, 0);
        } else if (*sc).tcb != 0 {
            refill_update(sc, now, (*sc).period, budget_ticks, max_refills);
        } else {
            refill_new(sc, max_refills, now, budget_ticks, (*sc).period);
        }
        if (*sc).tcb != 0 {
            let tcb = (*sc).tcb as *mut Tcb;
            Some((tcb, (*sc).core as u8))
        } else {
            None
        }
    };
    if let Some((tcb, core)) = affinity_update {
        unsafe {
            crate::object::tcb::set_affinity(tcb, core);
        }
    }
}

#[inline]
fn is_kernel_pspace_kva(kva: u64) -> bool {
    kva >= crate::abi::constants::PPTR_BASE as u64 && kva < crate::abi::constants::PPTR_TOP as u64
}

#[inline]
fn now_ticks() -> u64 {
    crate::arch::current::csr::time() as u64
}

#[inline]
unsafe fn refill_ptr(sc: *mut SchedContext, index: u64) -> *mut Refill {
    unsafe {
        (sc as *mut u8)
            .add(REFILL_ARRAY_OFFSET_BYTES)
            .cast::<Refill>()
            .add(index as usize)
    }
}

#[inline]
unsafe fn refill_head_ptr(sc: *mut SchedContext) -> *mut Refill {
    unsafe { refill_ptr(sc, (*sc).refill_head) }
}

#[inline]
unsafe fn refill_tail_ptr(sc: *mut SchedContext) -> *mut Refill {
    unsafe { refill_ptr(sc, (*sc).refill_tail) }
}

#[inline]
unsafe fn refill_next(sc: *mut SchedContext, index: u64) -> u64 {
    unsafe {
        if index + 1 >= (*sc).refill_max {
            0
        } else {
            index + 1
        }
    }
}

#[inline]
unsafe fn refill_size(sc: *mut SchedContext) -> u64 {
    unsafe {
        if (*sc).refill_max == 0 {
            return 0;
        }
        if (*sc).refill_head <= (*sc).refill_tail {
            (*sc).refill_tail - (*sc).refill_head + 1
        } else {
            (*sc).refill_tail + 1 + ((*sc).refill_max - (*sc).refill_head)
        }
    }
}

#[inline]
unsafe fn refill_single(sc: *mut SchedContext) -> bool {
    unsafe { (*sc).refill_head == (*sc).refill_tail }
}

#[inline]
unsafe fn refill_full(sc: *mut SchedContext) -> bool {
    unsafe { refill_size(sc) >= (*sc).refill_max }
}

#[inline]
unsafe fn refill_ready(sc: *mut SchedContext, now: u64) -> bool {
    unsafe { (*refill_head_ptr(sc)).time <= now }
}

#[inline]
unsafe fn refill_sufficient(sc: *mut SchedContext, usage: u64) -> bool {
    unsafe {
        let head_amount = (*refill_head_ptr(sc)).amount;
        head_amount >= usage.saturating_add(MIN_BUDGET_TICKS)
    }
}

unsafe fn refill_sum(sc: *mut SchedContext) -> u64 {
    unsafe {
        if (*sc).refill_max == 0 {
            return 0;
        }
        let mut sum = 0u64;
        let mut current = (*sc).refill_head;
        loop {
            sum = sum.saturating_add((*refill_ptr(sc, current)).amount);
            if current == (*sc).refill_tail {
                break;
            }
            current = refill_next(sc, current);
        }
        sum
    }
}

unsafe fn refill_add_tail(sc: *mut SchedContext, refill: Refill) {
    unsafe {
        if refill_full(sc) {
            return;
        }
        let new_tail = refill_next(sc, (*sc).refill_tail);
        (*sc).refill_tail = new_tail;
        *refill_tail_ptr(sc) = refill;
    }
}

unsafe fn refill_pop_head(sc: *mut SchedContext) -> Refill {
    unsafe {
        let refill = *refill_head_ptr(sc);
        if !refill_single(sc) {
            (*sc).refill_head = refill_next(sc, (*sc).refill_head);
        }
        refill
    }
}

unsafe fn maybe_add_empty_round_robin_tail(sc: *mut SchedContext) {
    unsafe {
        if (*sc).period == 0 && refill_single(sc) && (*sc).refill_max >= 2 {
            let time = (*refill_head_ptr(sc)).time;
            refill_add_tail(sc, Refill { time, amount: 0 });
        }
    }
}

unsafe fn refill_new(sc: *mut SchedContext, max_refills: u64, now: u64, budget: u64, period: u64) {
    unsafe {
        (*sc).period = period;
        (*sc).refill_max = max_refills.max(crate::abi::constants::SEL4_MIN_REFILLS);
        (*sc).refill_head = 0;
        (*sc).refill_tail = 0;
        *refill_head_ptr(sc) = Refill {
            time: now,
            amount: budget,
        };
        maybe_add_empty_round_robin_tail(sc);
    }
}

unsafe fn refill_update(
    sc: *mut SchedContext,
    now: u64,
    new_period: u64,
    new_budget: u64,
    new_max_refills: u64,
) {
    unsafe {
        let mut head = *refill_head_ptr(sc);
        if head.time <= now {
            head.time = now;
        }
        (*sc).refill_head = 0;
        (*sc).refill_tail = 0;
        (*sc).refill_max = new_max_refills.max(crate::abi::constants::SEL4_MIN_REFILLS);
        (*sc).period = new_period;
        if head.amount >= new_budget {
            head.amount = new_budget;
            *refill_head_ptr(sc) = head;
        } else {
            *refill_head_ptr(sc) = head;
            refill_add_tail(
                sc,
                Refill {
                    time: head.time.wrapping_add(new_period),
                    amount: new_budget - head.amount,
                },
            );
        }
        maybe_add_empty_round_robin_tail(sc);
    }
}

unsafe fn schedule_used(sc: *mut SchedContext, new: Refill) {
    unsafe {
        if new.amount == 0 {
            return;
        }
        let tail_ptr = refill_tail_ptr(sc);
        let tail = *tail_ptr;
        if tail.time.saturating_add(tail.amount) >= new.time {
            (*tail_ptr).amount = tail.amount.saturating_add(new.amount);
        } else if !refill_full(sc) {
            refill_add_tail(sc, new);
        } else {
            (*tail_ptr).time = new.time.saturating_sub(tail.amount);
            (*tail_ptr).amount = tail.amount.saturating_add(new.amount);
        }
    }
}

unsafe fn head_refill_overrun(sc: *mut SchedContext, usage: u64) -> bool {
    unsafe {
        let head = *refill_head_ptr(sc);
        head.amount != 0 && head.amount <= usage
    }
}

unsafe fn charge_entire_head_refill(sc: *mut SchedContext, usage: u64) -> u64 {
    unsafe {
        let head = *refill_head_ptr(sc);
        if refill_single(sc) {
            (*refill_head_ptr(sc)).time = head.time.wrapping_add((*sc).period);
        } else {
            let mut old_head = refill_pop_head(sc);
            old_head.time = old_head.time.wrapping_add((*sc).period);
            schedule_used(sc, old_head);
        }
        usage.saturating_sub(head.amount)
    }
}

unsafe fn merge_nonoverlapping_head_refill(sc: *mut SchedContext) {
    unsafe {
        if refill_single(sc) {
            return;
        }
        let head = refill_pop_head(sc);
        let next_head = refill_head_ptr(sc);
        (*next_head).amount = (*next_head).amount.saturating_add(head.amount);
        (*next_head).time = (*next_head).time.saturating_sub(head.amount);
    }
}

unsafe fn refill_head_overlapping(sc: *mut SchedContext) -> bool {
    unsafe {
        if refill_single(sc) {
            return false;
        }
        let head = *refill_head_ptr(sc);
        let next = *refill_ptr(sc, refill_next(sc, (*sc).refill_head));
        next.time <= head.time.saturating_add(head.amount)
    }
}

unsafe fn merge_overlapping_head_refill(sc: *mut SchedContext) {
    unsafe {
        if refill_single(sc) {
            return;
        }
        let old_head = refill_pop_head(sc);
        let head = refill_head_ptr(sc);
        (*head).time = old_head.time;
        (*head).amount = (*head).amount.saturating_add(old_head.amount);
    }
}

unsafe fn refill_unblock_check(sc: *mut SchedContext, now: u64) {
    unsafe {
        if (*sc).period == 0 || !refill_ready(sc, now) {
            return;
        }
        (*refill_head_ptr(sc)).time = now;
        while refill_head_overlapping(sc) {
            merge_overlapping_head_refill(sc);
        }
    }
}

unsafe fn refill_budget_check(sc: *mut SchedContext, usage: u64) {
    unsafe {
        let mut current_usage = usage;
        while head_refill_overrun(sc, current_usage) {
            current_usage = charge_entire_head_refill(sc, current_usage);
        }

        if current_usage > 0 {
            let head = *refill_head_ptr(sc);
            if head.amount > current_usage {
                *refill_head_ptr(sc) = Refill {
                    time: head.time.wrapping_add(current_usage),
                    amount: head.amount - current_usage,
                };
                schedule_used(
                    sc,
                    Refill {
                        time: head.time.wrapping_add((*sc).period),
                        amount: current_usage,
                    },
                );
            }
        }

        while (*refill_head_ptr(sc)).amount < MIN_BUDGET_TICKS && !refill_single(sc) {
            merge_nonoverlapping_head_refill(sc);
        }
    }
}

unsafe fn release_one(sc: *mut SchedContext, now: u64) -> bool {
    unsafe {
        if (*sc).period == 0 || (*sc).refill_max == 0 {
            return false;
        }
        refill_unblock_check(sc, now);
        refill_ready(sc, now) && refill_sufficient(sc, 0)
    }
}

#[inline]
fn budget_remains(remaining: u64, charge_ticks: u64, margin_ticks: u64) -> bool {
    remaining > charge_ticks.saturating_add(margin_ticks)
}

#[inline]
fn apply_timer_precision(deadline: u64, now: u64, precision_ticks: u64) -> u64 {
    let earliest = now.saturating_add(precision_ticks);
    if deadline <= earliest {
        now
    } else {
        deadline - precision_ticks
    }
}

#[inline]
fn round_robin_precision_ticks(total_budget: u64) -> u64 {
    if total_budget >= LONG_ROUND_ROBIN_BUDGET_THRESHOLD_TICKS {
        ROUND_ROBIN_TIMER_PRECISION_TICKS
    } else {
        TIMER_PRECISION_TICKS
    }
}

#[inline]
fn round_robin_budget_margin_ticks(total_budget: u64) -> u64 {
    if total_budget >= LONG_ROUND_ROBIN_BUDGET_THRESHOLD_TICKS {
        ROUND_ROBIN_TIMER_PRECISION_TICKS
    } else {
        MIN_BUDGET_TICKS
    }
}

unsafe fn current_budget_deadline(
    now: u64,
    last_budget_account_ticks: u64,
    current_tcb: *const Tcb,
) -> Option<u64> {
    let sc_kva = unsafe { tcb_sched_context(current_tcb as *mut Tcb) };
    if sc_kva == 0 || !is_kernel_pspace_kva(sc_kva) {
        return None;
    }
    let sc = sc_kva as *mut SchedContext;
    unsafe {
        let _guard = lock(sc_kva);
        if (*sc).refill_max == 0 {
            return None;
        }
        let head = *refill_head_ptr(sc);
        if head.time > now {
            return Some(apply_timer_precision(head.time, now, TIMER_PRECISION_TICKS));
        }
        let elapsed = now.saturating_sub(last_budget_account_ticks);
        let precision = if (*sc).period == 0 {
            let head = refill_head_ptr(sc);
            let tail = refill_tail_ptr(sc);
            round_robin_precision_ticks((*head).amount.saturating_add((*tail).amount))
        } else {
            TIMER_PRECISION_TICKS
        };
        Some(apply_timer_precision(
            now.saturating_add(head.amount.saturating_sub(elapsed)),
            now,
            precision,
        ))
    }
}

unsafe fn next_release_deadline(now: u64) -> Option<u64> {
    let (contexts, count) = release_sched_context_snapshot();
    let current_core = crate::kernel::smp::current_core_id();
    let mut deadline: Option<u64> = None;
    unsafe {
        let mut i = 0;
        while i < count {
            let sc_kva = contexts[i];
            if sc_kva != 0 && is_kernel_pspace_kva(sc_kva) {
                let sc = sc_kva as *mut SchedContext;
                let _guard = lock(sc_kva);
                if (*sc).core as usize == current_core
                    && (*sc).period != 0
                    && (*sc).refill_max != 0
                    && (*sc).tcb != 0
                {
                    let head = *refill_head_ptr(sc);
                    let candidate = if head.time <= now { now } else { head.time };
                    let candidate = apply_timer_precision(candidate, now, TIMER_PRECISION_TICKS);
                    deadline = Some(deadline.map_or(candidate, |current| current.min(candidate)));
                }
            }
            i += 1;
        }
    }
    deadline
}

pub unsafe fn scheduler_timer_deadline(
    now: u64,
    last_budget_account_ticks: u64,
    current_tcb: *const Tcb,
) -> Option<u64> {
    let mut deadline =
        unsafe { current_budget_deadline(now, last_budget_account_ticks, current_tcb) };
    if let Some(release_deadline) = unsafe { next_release_deadline(now) } {
        deadline = Some(deadline.map_or(release_deadline, |current| current.min(release_deadline)));
    }
    deadline
}

pub unsafe fn release_due(now: u64) {
    let (contexts, count) = release_sched_context_snapshot();
    let current_core = crate::kernel::smp::current_core_id();
    let current = tcb::current();
    unsafe {
        let mut i = 0;
        while i < count {
            let sc_kva = contexts[i];
            let mut wake_tcb = core::ptr::null_mut();
            if sc_kva != 0 && is_kernel_pspace_kva(sc_kva) {
                let sc = sc_kva as *mut SchedContext;
                {
                    let _guard = lock(sc_kva);
                    if (*sc).core as usize == current_core
                        && (*sc).tcb != 0
                        && (*sc).tcb != current as u64
                        && release_one(sc, now)
                    {
                        let tcb = (*sc).tcb as *mut Tcb;
                        if tcb::runnable_sched_context_snapshot(tcb).0 {
                            wake_tcb = tcb;
                        }
                    }
                }
                if !wake_tcb.is_null() {
                    unregister_release(sc_kva);
                    crate::object::tcb::enqueue(wake_tcb);
                }
            }
            i += 1;
        }
    }
}

pub unsafe fn postpone(sc_kva: u64) {
    if sc_kva == 0 || !is_kernel_pspace_kva(sc_kva) {
        return;
    }
    let tcb = unsafe {
        let sc = sc_kva as *mut SchedContext;
        let _guard = lock(sc_kva);
        register_release(sc_kva);
        (*sc).tcb as *mut Tcb
    };
    if !tcb.is_null() {
        unsafe {
            crate::object::tcb::dequeue(tcb);
        }
    }
}

pub unsafe fn postpone_unreleased_tcb(tcb: *mut Tcb) -> bool {
    if tcb.is_null() {
        return false;
    }
    let (runnable, sc_kva) = tcb::runnable_sched_context_snapshot(tcb);
    if !runnable || sc_kva == 0 || !is_kernel_pspace_kva(sc_kva) {
        return false;
    }
    let sc = sc_kva as *mut SchedContext;
    let should_postpone = unsafe {
        let _guard = lock(sc_kva);
        if (*sc).refill_max == 0 || (*sc).period == 0 {
            false
        } else {
            let now = now_ticks();
            refill_unblock_check(sc, now);
            !refill_ready(sc, now) || !refill_sufficient(sc, 0)
        }
    };
    if should_postpone {
        unsafe {
            postpone(sc_kva);
        }
    }
    should_postpone
}

pub unsafe fn has_budget(sc_kva: u64) -> bool {
    if sc_kva == 0 {
        return false;
    }
    if !is_kernel_pspace_kva(sc_kva) {
        return true;
    }
    let sc = sc_kva as *mut SchedContext;
    unsafe {
        let _guard = lock(sc_kva);
        if (*sc).refill_max == 0 {
            return false;
        }
        let now = now_ticks();
        if (*sc).period != 0 {
            refill_unblock_check(sc, now);
        }
        (*sc).period == 0 || (refill_ready(sc, now) && refill_sufficient(sc, 0))
    }
}

pub unsafe fn is_round_robin(sc_kva: u64) -> bool {
    if sc_kva == 0 || !is_kernel_pspace_kva(sc_kva) {
        return false;
    }
    let sc = sc_kva as *mut SchedContext;
    unsafe {
        let _guard = lock(sc_kva);
        (*sc).refill_max != 0 && (*sc).period == 0
    }
}

pub unsafe fn tcb_schedulable(tcb: *mut Tcb) -> bool {
    if tcb.is_null() {
        return false;
    }
    let (runnable, sched_context) = tcb::runnable_sched_context_snapshot(tcb);
    runnable && sched_context != 0 && unsafe { has_budget(sched_context) }
}

fn tcb_blocked(tcb: *mut Tcb) -> bool {
    let (state, _) = tcb::wait_state_snapshot(tcb);
    state == tcb::ThreadState::BlockedOnReceive as u8
        || state == tcb::ThreadState::BlockedOnSend as u8
        || state == tcb::ThreadState::BlockedOnReply as u8
        || state == tcb::ThreadState::BlockedOnNotification as u8
}

unsafe fn released_locked(sc: *mut SchedContext, sc_kva: u64) -> bool {
    if !is_kernel_pspace_kva(sc_kva) {
        return true;
    }
    unsafe {
        if (*sc).refill_max == 0 {
            return false;
        }
        let now = now_ticks();
        if (*sc).period != 0 {
            refill_unblock_check(sc, now);
        }
        refill_ready(sc, now) && refill_sufficient(sc, 0)
    }
}

pub unsafe fn charge(sc_kva: u64, ticks: u64) -> bool {
    if sc_kva == 0 {
        return false;
    }
    if !is_kernel_pspace_kva(sc_kva) {
        return true;
    }
    let sc = sc_kva as *mut SchedContext;
    unsafe {
        let _guard = lock(sc_kva);
        if (*sc).refill_max == 0 {
            return false;
        }
        (*sc).consumed = (*sc).consumed.saturating_add(ticks);
        if (*sc).period == 0 {
            let head = refill_head_ptr(sc);
            let tail = refill_tail_ptr(sc);
            let total_budget = (*head).amount.saturating_add((*tail).amount);
            if budget_remains(
                (*head).amount,
                ticks,
                round_robin_budget_margin_ticks(total_budget),
            ) {
                (*head).amount -= ticks;
                (*tail).amount = (*tail).amount.saturating_add(ticks);
                return true;
            }
            (*head).amount = (*head).amount.saturating_add((*tail).amount);
            (*tail).amount = 0;
            return false;
        }

        let now = now_ticks();
        if !refill_ready(sc, now) || !refill_sufficient(sc, 0) {
            register_release(sc_kva);
            if (*sc).tcb != 0 {
                crate::object::tcb::dequeue((*sc).tcb as *mut Tcb);
            }
            return false;
        }

        let head_amount = (*refill_head_ptr(sc)).amount;
        if budget_remains(head_amount, ticks, MIN_BUDGET_TICKS) {
            refill_budget_check(sc, ticks);
            return true;
        }

        refill_budget_check(sc, head_amount);
        register_release(sc_kva);
        if (*sc).tcb != 0 {
            let tcb = (*sc).tcb as *mut Tcb;
            crate::object::tcb::dequeue(tcb);
        }
        false
    }
}

pub unsafe fn replenish(sc_kva: u64) {
    if sc_kva == 0 || !is_kernel_pspace_kva(sc_kva) {
        return;
    }
    let sc = sc_kva as *mut SchedContext;
    unsafe {
        let _guard = lock(sc_kva);
        if (*sc).period != 0 {
            let budget = refill_sum(sc);
            refill_new(sc, (*sc).refill_max, now_ticks(), budget, (*sc).period);
        }
    }
}

pub unsafe fn yield_tcb(tcb: *mut Tcb) -> bool {
    if tcb.is_null() {
        return false;
    }
    let sc_kva = unsafe { tcb_sched_context(tcb) };
    if sc_kva == 0 || !is_kernel_pspace_kva(sc_kva) {
        return false;
    }
    let sc = sc_kva as *mut SchedContext;
    unsafe {
        let _guard = lock(sc_kva);
        if (*sc).period == 0 {
            return false;
        }
        let now = now_ticks();
        refill_unblock_check(sc, now);
        let head_amount = (*refill_head_ptr(sc)).amount;
        refill_budget_check(sc, head_amount);
        register_release(sc_kva);
        if (*sc).tcb != 0 {
            crate::object::tcb::dequeue((*sc).tcb as *mut Tcb);
        }
    }
    true
}

pub unsafe fn charge_tcb(tcb: *mut Tcb, ticks: u64) -> bool {
    if tcb.is_null() {
        return false;
    }
    let sc = unsafe { tcb_sched_context(tcb) };
    unsafe { charge(sc, ticks) }
}

pub unsafe fn consume_consumed(sc_kva: u64) -> u64 {
    if sc_kva == 0 || !is_kernel_pspace_kva(sc_kva) {
        return 0;
    }
    let sc = sc_kva as *mut SchedContext;
    unsafe {
        let _guard = lock(sc_kva);
        consume_consumed_locked(sc)
    }
}

#[inline]
unsafe fn consume_consumed_locked(sc: *mut SchedContext) -> u64 {
    unsafe {
        let consumed_ticks = (*sc).consumed;
        (*sc).consumed = 0;
        consumed_ticks / crate::abi::constants::SEL4_TIMER_TICKS_PER_US
    }
}

pub unsafe fn badge_and_consume_consumed(sc_kva: u64) -> (u64, u64) {
    if sc_kva == 0 || !is_kernel_pspace_kva(sc_kva) {
        return (0, 0);
    }
    let sc = sc_kva as *mut SchedContext;
    unsafe {
        let _guard = lock(sc_kva);
        let badge = (*sc).badge;
        let consumed_ticks = (*sc).consumed;
        (*sc).consumed = 0;
        (
            badge,
            consumed_ticks / crate::abi::constants::SEL4_TIMER_TICKS_PER_US,
        )
    }
}

pub unsafe fn bind_tcb(sc_kva: u64, tcb: *mut Tcb) -> bool {
    if sc_kva == 0 || tcb.is_null() {
        return false;
    }
    let sc = sc_kva as *mut SchedContext;
    let affinity = unsafe {
        let _guard = lock(sc_kva);
        if (*sc).tcb != 0 && (*sc).tcb != tcb as u64 {
            return false;
        }
        if !tcb::bind_sched_context_if_unbound_or_same(tcb, sc_kva) {
            return false;
        }
        (*sc).tcb = tcb as u64;
        (*sc).core as u8
    };
    unsafe {
        crate::object::tcb::set_affinity(tcb, affinity);
    }
    true
}

pub unsafe fn try_bind_tcb(sc_kva: u64, tcb: *mut Tcb) -> bool {
    if sc_kva == 0 || tcb.is_null() {
        return false;
    }
    let sc = sc_kva as *mut SchedContext;
    let target_blocked = tcb_blocked(tcb);
    let affinity = unsafe {
        let _guard = lock(sc_kva);
        if (*sc).tcb != 0 {
            return false;
        }
        if target_blocked && !released_locked(sc, sc_kva) {
            return false;
        }
        if !tcb::bind_sched_context_if_unbound(tcb, sc_kva) {
            return false;
        }
        (*sc).tcb = tcb as u64;
        (*sc).core as u8
    };
    unsafe {
        crate::object::tcb::set_affinity(tcb, affinity);
    }
    true
}

pub unsafe fn is_bound_to_tcb(sc_kva: u64, tcb: *mut Tcb) -> bool {
    if sc_kva == 0 || tcb.is_null() {
        return false;
    }
    let sc = sc_kva as *mut SchedContext;
    unsafe {
        let _guard = lock(sc_kva);
        (*sc).tcb == tcb as u64
    }
}

pub unsafe fn can_set_sched_params_to_tcb(sc_kva: u64, tcb: *mut Tcb, current_sc: u64) -> bool {
    if sc_kva == 0 || tcb.is_null() {
        return false;
    }
    let target_blocked = tcb_blocked(tcb);
    let sc = sc_kva as *mut SchedContext;
    unsafe {
        let _guard = lock(sc_kva);
        if current_sc != sc_kva {
            if current_sc != 0 {
                return false;
            }
            if (*sc).tcb != 0 {
                return false;
            }
        }
        if target_blocked && !released_locked(sc, sc_kva) {
            return false;
        }
    }
    true
}

pub unsafe fn bound_tcb(sc_kva: u64) -> *mut Tcb {
    if sc_kva == 0 {
        return core::ptr::null_mut();
    }
    let sc = sc_kva as *mut SchedContext;
    unsafe {
        let _guard = lock(sc_kva);
        (*sc).tcb as *mut Tcb
    }
}

pub unsafe fn clear_tcb_if_bound(sc_kva: u64, tcb: *mut Tcb) -> bool {
    if sc_kva == 0 || tcb.is_null() {
        return false;
    }
    let sc = sc_kva as *mut SchedContext;
    unsafe {
        let _guard = lock(sc_kva);
        if (*sc).tcb != tcb as u64 {
            return false;
        }
        let _ = tcb::clear_sched_context_if(tcb, sc_kva);
        (*sc).tcb = 0;
        true
    }
}

pub unsafe fn donate_if_unbound(sc_kva: u64, to: *mut Tcb) -> bool {
    if sc_kva == 0 || to.is_null() {
        return false;
    }
    let sc = sc_kva as *mut SchedContext;
    unsafe {
        let _guard = lock(sc_kva);
        if (*sc).tcb != 0 {
            return false;
        }
        if !tcb::bind_sched_context_if_unbound(to, sc_kva) {
            return false;
        }
        (*sc).tcb = to as u64;
        true
    }
}

pub unsafe fn return_if_bound_to(sc_kva: u64, tcb: *mut Tcb) -> bool {
    if sc_kva == 0 || tcb.is_null() {
        return false;
    }
    let sc = sc_kva as *mut SchedContext;
    unsafe {
        let _guard = lock(sc_kva);
        if (*sc).tcb != tcb as u64 {
            return false;
        }
        if !tcb::clear_sched_context_if(tcb, sc_kva) {
            return false;
        }
        (*sc).tcb = 0;
        true
    }
}

pub unsafe fn try_unbind_tcb(sc_kva: u64, tcb: *mut Tcb) -> bool {
    if sc_kva == 0 || tcb.is_null() {
        return false;
    }
    crate::kernel::smp::remote_tcb_stall(tcb);
    let sc = sc_kva as *mut SchedContext;
    let unbound = unsafe {
        let _guard = lock(sc_kva);
        if (*sc).tcb != tcb as u64 {
            return false;
        }
        crate::object::tcb::dequeue(tcb);
        if (*sc).yield_from != 0 {
            tcb::clear_yield_from_if(tcb, (*sc).yield_from as *mut Tcb);
        }
        let _ = tcb::clear_sched_context_if(tcb, sc_kva);
        (*sc).tcb = 0;
        true
    };
    if unbound {
        unregister_release(sc_kva);
    }
    unbound
}

pub unsafe fn replace_reply_head(sc_kva: u64, reply_kva: u64) -> u64 {
    if sc_kva == 0 {
        return 0;
    }
    let sc = sc_kva as *mut SchedContext;
    unsafe {
        let _guard = lock(sc_kva);
        let old_head = (*sc).reply;
        (*sc).reply = reply_kva;
        old_head
    }
}

pub unsafe fn push_reply_and_donate(sc_kva: u64, to: *mut Tcb, reply_kva: u64) -> Option<u64> {
    if sc_kva == 0 || to.is_null() || reply_kva == 0 {
        return None;
    }
    let sc = sc_kva as *mut SchedContext;
    unsafe {
        let _guard = lock(sc_kva);
        let old_head = (*sc).reply;
        if (*sc).tcb != 0 {
            let from = (*sc).tcb as *mut Tcb;
            if !tcb::move_sched_context_if_target_unbound(from, to, sc_kva) {
                return None;
            }
            crate::object::tcb::dequeue(from);
        } else if !tcb::bind_sched_context_if_unbound(to, sc_kva) {
            return None;
        }
        (*sc).reply = reply_kva;
        (*sc).tcb = to as u64;
        Some(old_head)
    }
}

pub unsafe fn set_reply_head(sc_kva: u64, reply_kva: u64) {
    if sc_kva == 0 {
        return;
    }
    let sc = sc_kva as *mut SchedContext;
    unsafe {
        let _guard = lock(sc_kva);
        (*sc).reply = reply_kva;
    }
}

pub unsafe fn clear_reply_head(sc_kva: u64) {
    unsafe {
        set_reply_head(sc_kva, 0);
    }
}

pub unsafe fn cancel_yield_to(yielder: *mut Tcb) {
    if yielder.is_null() {
        return;
    }
    unsafe {
        let sc_kva = tcb::yield_to_sched_context_snapshot(yielder);
        if sc_kva != 0 && is_kernel_pspace_kva(sc_kva) {
            let sc = sc_kva as *mut SchedContext;
            let _guard = lock(sc_kva);
            let target = (*sc).tcb as *mut Tcb;
            if (*sc).yield_from == yielder as u64 {
                (*sc).yield_from = 0;
            }
            tcb::cancel_yield_to_for_target(yielder, target);
        } else {
            tcb::clear_yield_to(yielder);
        }
    }
}

pub unsafe fn start_yield_to(sc_kva: u64, yielder: *mut Tcb, target: *mut Tcb) -> bool {
    if sc_kva == 0 || yielder.is_null() || target.is_null() {
        return false;
    }
    let sc = sc_kva as *mut SchedContext;
    unsafe {
        let _guard = lock(sc_kva);
        if (*sc).tcb != target as u64 {
            return false;
        }
        if (*sc).yield_from != 0 {
            return false;
        }
        if !tcb::start_yield_to_if_idle(yielder, target, sc_kva, (*sc).consumed) {
            return false;
        }
        (*sc).yield_from = yielder as u64;
        true
    }
}

pub unsafe fn complete_yield_to_target(target: *mut Tcb) {
    if target.is_null() {
        return;
    }
    unsafe {
        let yielder = tcb::yield_from_snapshot(target);
        if yielder.is_null() {
            return;
        }
        let sc_kva = tcb::yield_to_sched_context_snapshot(yielder);
        let completed = if sc_kva != 0 && is_kernel_pspace_kva(sc_kva) {
            let sc = sc_kva as *mut SchedContext;
            let _guard = lock(sc_kva);
            if !tcb::yield_to_pair_matches(yielder, target, sc_kva) {
                return;
            }
            let consumed = consume_consumed_locked(sc);
            let completed = tcb::finish_yield_to_pair(yielder, target, sc_kva, consumed);
            if completed && (*sc).yield_from == yielder as u64 {
                (*sc).yield_from = 0;
            }
            completed
        } else {
            tcb::finish_yield_to_pair(yielder, target, sc_kva, 0)
        };
        if completed {
            tcb::enqueue(yielder);
        }
    }
}

pub unsafe fn complete_yield_to_yielder(sc_kva: u64) {
    if sc_kva == 0 {
        return;
    }
    let sc = sc_kva as *mut SchedContext;
    let (yielder, target, consumed) = unsafe {
        let _guard = lock(sc_kva);
        let yielder = (*sc).yield_from as *mut Tcb;
        if yielder.is_null() {
            return;
        }
        let target = (*sc).tcb as *mut Tcb;
        let consumed = consume_consumed_locked(sc);
        (*sc).yield_from = 0;
        (yielder, target, consumed)
    };
    unsafe {
        if !target.is_null() {
            tcb::clear_yield_from_if(target, yielder);
        }
        if tcb::finish_yield_to(yielder, sc_kva, consumed) {
            tcb::enqueue(yielder);
        }
    }
}

pub unsafe fn donate(sc_kva: u64, to: *mut Tcb) -> bool {
    if sc_kva == 0 || to.is_null() {
        return false;
    }
    unsafe {
        let sc = sc_kva as *mut SchedContext;
        let _guard = lock(sc_kva);
        if (*sc).tcb != 0 {
            let from = (*sc).tcb as *mut Tcb;
            if !tcb::move_sched_context_if_target_unbound(from, to, sc_kva) {
                return false;
            }
            crate::object::tcb::dequeue(from);
        } else if !tcb::bind_sched_context_if_unbound(to, sc_kva) {
            return false;
        }
        (*sc).tcb = to as u64;
        true
    }
}

pub unsafe fn try_bind_notification(sc_kva: u64, ntfn_kva: u64) -> bool {
    if sc_kva == 0 || ntfn_kva == 0 {
        return false;
    }
    let sc = sc_kva as *mut SchedContext;
    let ntfn = ntfn_kva as *mut crate::object::notification::Notification;
    let _guard = unsafe { crate::object::notification::lock_queue(ntfn) };
    unsafe {
        let _sc_guard = lock(sc_kva);
        if (*sc).notification != 0 || (*ntfn).sched_context() != 0 {
            return false;
        }
        (*ntfn).set_sched_context(sc_kva);
        (*sc).notification = ntfn_kva;
    }
    true
}

pub unsafe fn bind_notification(sc_kva: u64, ntfn_kva: u64) {
    let _ = unsafe { try_bind_notification(sc_kva, ntfn_kva) };
}

pub unsafe fn try_unbind_notification(sc_kva: u64, ntfn_kva: u64) -> bool {
    if sc_kva == 0 || ntfn_kva == 0 {
        return false;
    }
    let sc = sc_kva as *mut SchedContext;
    let ntfn = ntfn_kva as *mut crate::object::notification::Notification;
    let _guard = unsafe { crate::object::notification::lock_queue(ntfn) };
    unsafe {
        let _sc_guard = lock(sc_kva);
        if (*sc).notification != ntfn_kva {
            return false;
        }
        (*ntfn).set_sched_context(0);
        (*sc).notification = 0;
    }
    true
}

unsafe fn unbind_tcb_for_sc(sc_kva: u64) -> *mut Tcb {
    if sc_kva == 0 {
        return core::ptr::null_mut();
    }
    let sc = sc_kva as *mut SchedContext;
    let bound_tcb = unsafe {
        let _guard = lock(sc_kva);
        (*sc).tcb as *mut Tcb
    };
    if bound_tcb.is_null() {
        return core::ptr::null_mut();
    }
    crate::kernel::smp::remote_tcb_stall(bound_tcb);
    unsafe {
        if try_unbind_tcb(sc_kva, bound_tcb) {
            bound_tcb
        } else {
            core::ptr::null_mut()
        }
    }
}

unsafe fn unbind_notification_for_sc(sc_kva: u64) {
    if sc_kva == 0 {
        return;
    }
    let sc = sc_kva as *mut SchedContext;
    let bound_ntfn = unsafe {
        let _guard = lock(sc_kva);
        (*sc).notification as *mut crate::object::notification::Notification
    };
    if bound_ntfn.is_null() {
        return;
    }
    unsafe {
        let _ntfn_guard = crate::object::notification::lock_queue(bound_ntfn);
        let _sc_guard = lock(sc_kva);
        if (*sc).notification == bound_ntfn as u64 {
            (*bound_ntfn).set_sched_context(0);
            (*sc).notification = 0;
        }
    }
}

unsafe fn unbind_reply_for_sc(sc_kva: u64) {
    if sc_kva == 0 {
        return;
    }
    let sc = sc_kva as *mut SchedContext;
    let reply_head = unsafe {
        let _guard = lock(sc_kva);
        let reply_head = (*sc).reply;
        (*sc).reply = 0;
        reply_head
    };
    if reply_head != 0 {
        unsafe {
            crate::object::reply::clear_next(reply_head);
        }
    }
}

pub(crate) unsafe fn clear_notification_if_bound(sc_kva: u64, ntfn_kva: u64) -> bool {
    if sc_kva == 0 || ntfn_kva == 0 {
        return false;
    }
    let sc = sc_kva as *mut SchedContext;
    unsafe {
        let _guard = lock(sc_kva);
        if (*sc).notification != ntfn_kva {
            return false;
        }
        (*sc).notification = 0;
    }
    true
}

pub unsafe fn unbind(sc_kva: u64) {
    if sc_kva == 0 {
        return;
    }
    unsafe {
        let wake_tcb = unbind_tcb_for_sc(sc_kva);
        unbind_notification_for_sc(sc_kva);
        unbind_reply_for_sc(sc_kva);
        crate::kernel::smp::wake_current_core_of_tcb(wake_tcb);
    }
}

pub unsafe fn finalize(sc_kva: u64) {
    crate::kernel::smp::debug_assert_kernel_lock_held();
    if sc_kva == 0 {
        return;
    }
    let sc = sc_kva as *mut SchedContext;
    let wake_tcb = unsafe { unbind_tcb_for_sc(sc_kva) };
    unsafe {
        unbind_notification_for_sc(sc_kva);
        unbind_reply_for_sc(sc_kva);
        complete_yield_to_yielder(sc_kva);
        let _guard = lock(sc_kva);
        (*sc).refill_max = 0;
        (*sc).sporadic = 0;
        // Rust-local release tracking is removed after the seL4-visible
        // finalise ordering has made the SC invalid.
        unregister(sc_kva);
    }
    crate::kernel::smp::wake_current_core_of_tcb(wake_tcb);
}
