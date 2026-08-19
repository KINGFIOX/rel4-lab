use core::future::Future;
use core::pin::Pin;
use core::sync::atomic::{AtomicBool, Ordering};
use core::task::{Context, Poll, Waker};

use crate::sync::SpinLock;

struct WaitCellInner<T> {
    value: Option<T>,
    waker: Option<Waker>,
}

/// One-shot completion cell. `const` constructible for static tables.
pub struct WaitCell<T> {
    inner: SpinLock<WaitCellInner<T>>,
}

unsafe impl<T: Send> Send for WaitCell<T> {}
unsafe impl<T: Send> Sync for WaitCell<T> {}

impl<T> WaitCell<T> {
    pub const fn new() -> Self {
        Self {
            inner: SpinLock::new(WaitCellInner {
                value: None,
                waker: None,
            }),
        }
    }

    pub fn reset(&self) {
        let mut inner = self.inner.lock();
        inner.value = None;
        inner.waker = None;
    }

    pub fn complete(&self, value: T) {
        let waker = {
            let mut inner = self.inner.lock();
            inner.value = Some(value);
            inner.waker.take()
        };
        if let Some(waker) = waker {
            waker.wake();
        }
    }

    pub fn wait(&self) -> WaitFuture<'_, T> {
        WaitFuture { cell: self }
    }
}

pub struct WaitFuture<'a, T> {
    cell: &'a WaitCell<T>,
}

impl<T> Future for WaitFuture<'_, T> {
    type Output = T;

    fn poll(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Self::Output> {
        let mut inner = self.cell.inner.lock();
        if let Some(value) = inner.value.take() {
            return Poll::Ready(value);
        }
        inner.waker = Some(cx.waker().clone());
        Poll::Pending
    }
}

const MAX_LOCK_WAITERS: usize = 96;

struct AsyncLockWaiters {
    wakers: [Option<Waker>; MAX_LOCK_WAITERS],
}

/// Async mutex whose permit is `Send` and may be held across `.await`.
pub struct AsyncLock {
    locked: AtomicBool,
    waiters: SpinLock<AsyncLockWaiters>,
}

pub struct AsyncLockGuard<'a> {
    lock: &'a AsyncLock,
}

unsafe impl Send for AsyncLock {}
unsafe impl Sync for AsyncLock {}
unsafe impl Send for AsyncLockGuard<'_> {}

impl AsyncLock {
    pub const fn new() -> Self {
        Self {
            locked: AtomicBool::new(false),
            waiters: SpinLock::new(AsyncLockWaiters {
                wakers: [const { None }; MAX_LOCK_WAITERS],
            }),
        }
    }

    pub fn lock(&self) -> AsyncLockFuture<'_> {
        AsyncLockFuture { lock: self }
    }

    fn try_lock(&self) -> bool {
        self.locked
            .compare_exchange(false, true, Ordering::Acquire, Ordering::Relaxed)
            .is_ok()
    }

    fn register(&self, waker: &Waker) {
        let mut waiters = self.waiters.lock();
        let mut i = 0usize;
        while i < MAX_LOCK_WAITERS {
            if waiters.wakers[i].is_none() {
                waiters.wakers[i] = Some(waker.clone());
                return;
            }
            i += 1;
        }
        waker.wake_by_ref();
    }

    fn wake_one(&self) {
        let mut woken = None;
        {
            let mut waiters = self.waiters.lock();
            let mut i = 0usize;
            while i < MAX_LOCK_WAITERS {
                if let Some(waker) = waiters.wakers[i].take() {
                    woken = Some(waker);
                    break;
                }
                i += 1;
            }
        }
        if let Some(waker) = woken {
            waker.wake();
        }
    }

    fn release(&self) {
        self.locked.store(false, Ordering::Release);
        self.wake_one();
    }
}

pub struct AsyncLockFuture<'a> {
    lock: &'a AsyncLock,
}

impl<'a> Future for AsyncLockFuture<'a> {
    type Output = AsyncLockGuard<'a>;

    fn poll(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Self::Output> {
        if self.lock.try_lock() {
            return Poll::Ready(AsyncLockGuard { lock: self.lock });
        }
        self.lock.register(cx.waker());
        if self.lock.try_lock() {
            return Poll::Ready(AsyncLockGuard { lock: self.lock });
        }
        Poll::Pending
    }
}

impl Drop for AsyncLockGuard<'_> {
    fn drop(&mut self) {
        self.lock.release();
    }
}
