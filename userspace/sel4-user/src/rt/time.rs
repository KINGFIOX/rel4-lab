use core::future::Future;
use core::pin::Pin;
use core::sync::atomic::{AtomicU64, Ordering};
use core::task::{Context, Poll, Waker};

use crate::sync::SpinLock;

const MAX_TIMERS: usize = 96;

struct Timer {
    active: bool,
    deadline: u64,
    waker: Option<Waker>,
}

impl Timer {
    const fn empty() -> Self {
        Self {
            active: false,
            deadline: 0,
            waker: None,
        }
    }
}

static NOW: AtomicU64 = AtomicU64::new(0);
static TIMERS: SpinLock<[Timer; MAX_TIMERS]> =
    SpinLock::new([const { Timer::empty() }; MAX_TIMERS]);

pub fn now() -> u64 {
    NOW.load(Ordering::Relaxed)
}

pub fn advance(ticks: u64) {
    NOW.store(ticks, Ordering::Relaxed);
    let mut ready = [const { None::<Waker> }; MAX_TIMERS];
    let mut ready_len = 0usize;
    {
        let mut timers = TIMERS.lock();
        let mut i = 0usize;
        while i < MAX_TIMERS {
            if timers[i].active && ticks >= timers[i].deadline {
                timers[i].active = false;
                if let Some(waker) = timers[i].waker.take() {
                    ready[ready_len] = Some(waker);
                    ready_len += 1;
                }
            }
            i += 1;
        }
    }
    let mut i = 0usize;
    while i < ready_len {
        if let Some(waker) = ready[i].take() {
            waker.wake();
        }
        i += 1;
    }
}

pub fn sleep_until(deadline: u64) -> SleepUntil {
    SleepUntil {
        deadline,
        slot: None,
    }
}

pub struct SleepUntil {
    deadline: u64,
    slot: Option<usize>,
}

impl Future for SleepUntil {
    type Output = ();

    fn poll(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Self::Output> {
        let this = unsafe { self.get_unchecked_mut() };
        if now() >= this.deadline {
            this.release_slot();
            return Poll::Ready(());
        }
        if this.slot.is_none() {
            this.slot = alloc_timer(this.deadline, cx.waker());
        } else if let Some(slot) = this.slot {
            let mut timers = TIMERS.lock();
            if timers[slot].active {
                timers[slot].deadline = this.deadline;
                timers[slot].waker = Some(cx.waker().clone());
            }
        }
        if now() >= this.deadline {
            this.release_slot();
            return Poll::Ready(());
        }
        Poll::Pending
    }
}

impl Drop for SleepUntil {
    fn drop(&mut self) {
        self.release_slot();
    }
}

impl SleepUntil {
    fn release_slot(&mut self) {
        if let Some(slot) = self.slot.take() {
            let mut timers = TIMERS.lock();
            timers[slot].active = false;
            timers[slot].waker = None;
        }
    }
}

fn alloc_timer(deadline: u64, waker: &Waker) -> Option<usize> {
    let mut timers = TIMERS.lock();
    let mut i = 0usize;
    while i < MAX_TIMERS {
        if !timers[i].active {
            timers[i].active = true;
            timers[i].deadline = deadline;
            timers[i].waker = Some(waker.clone());
            return Some(i);
        }
        i += 1;
    }
    waker.wake_by_ref();
    None
}
