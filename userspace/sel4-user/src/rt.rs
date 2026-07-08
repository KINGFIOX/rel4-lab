use core::future::Future;
use core::pin::Pin;
use core::task::{Context, Poll, RawWaker, RawWakerVTable, Waker};

use crate::{
    IpcMessage, sel4_recv, sel4_recv_with_reply, sel4_reply_recv, sel4_reply_recv_with_reply,
    sel4_yield,
};

pub fn block_on<F>(mut future: F) -> F::Output
where
    F: Future,
{
    let waker = unsafe { Waker::from_raw(noop_raw_waker()) };
    let mut cx = Context::from_waker(&waker);
    let mut future = unsafe { Pin::new_unchecked(&mut future) };
    loop {
        match future.as_mut().poll(&mut cx) {
            Poll::Ready(value) => return value,
            Poll::Pending => unsafe { sel4_yield() },
        }
    }
}

pub fn recv(cptr: u64) -> RecvFuture {
    RecvFuture { cptr, reply: 0 }
}

pub fn recv_with_reply(cptr: u64, reply: u64) -> RecvFuture {
    RecvFuture { cptr, reply }
}

pub struct RecvFuture {
    cptr: u64,
    reply: u64,
}

impl Future for RecvFuture {
    type Output = IpcMessage;

    fn poll(self: Pin<&mut Self>, _cx: &mut Context<'_>) -> Poll<Self::Output> {
        if self.reply == 0 {
            Poll::Ready(unsafe { sel4_recv(self.cptr) })
        } else {
            Poll::Ready(unsafe { sel4_recv_with_reply(self.cptr, self.reply) })
        }
    }
}

pub fn reply_recv(cptr: u64, info: u64, mrs: &[u64]) -> ReplyRecvFuture {
    reply_recv_with_reply(cptr, info, mrs, 0)
}

pub fn reply_recv_with_reply(cptr: u64, info: u64, mrs: &[u64], reply: u64) -> ReplyRecvFuture {
    let mut saved_mrs = [0u64; 64];
    let len = core::cmp::min(mrs.len(), saved_mrs.len());
    let mut i = 0usize;
    while i < len {
        saved_mrs[i] = mrs[i];
        i += 1;
    }
    ReplyRecvFuture {
        cptr,
        info,
        mrs: saved_mrs,
        len,
        reply,
    }
}

pub struct ReplyRecvFuture {
    cptr: u64,
    info: u64,
    mrs: [u64; 64],
    len: usize,
    reply: u64,
}

impl Future for ReplyRecvFuture {
    type Output = IpcMessage;

    fn poll(self: Pin<&mut Self>, _cx: &mut Context<'_>) -> Poll<Self::Output> {
        if self.reply == 0 {
            Poll::Ready(unsafe { sel4_reply_recv(self.cptr, self.info, &self.mrs[..self.len]) })
        } else {
            Poll::Ready(unsafe {
                sel4_reply_recv_with_reply(self.cptr, self.info, &self.mrs[..self.len], self.reply)
            })
        }
    }
}

pub fn yield_now() -> YieldNow {
    YieldNow
}

pub struct YieldNow;

impl Future for YieldNow {
    type Output = ();

    fn poll(self: Pin<&mut Self>, _cx: &mut Context<'_>) -> Poll<Self::Output> {
        unsafe { sel4_yield() };
        Poll::Ready(())
    }
}

fn noop_raw_waker() -> RawWaker {
    RawWaker::new(core::ptr::null(), &NOOP_WAKER_VTABLE)
}

static NOOP_WAKER_VTABLE: RawWakerVTable =
    RawWakerVTable::new(noop_clone, noop_wake, noop_wake_by_ref, noop_drop);

fn noop_clone(_: *const ()) -> RawWaker {
    noop_raw_waker()
}

fn noop_wake(_: *const ()) {}

fn noop_wake_by_ref(_: *const ()) {}

fn noop_drop(_: *const ()) {}
