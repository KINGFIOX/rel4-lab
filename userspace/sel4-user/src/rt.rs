mod executor;
pub mod time;
mod wait;

use core::future::Future;
use core::pin::Pin;
use core::task::{Context, Poll};

use crate::{
    IpcMessage, sel4_recv, sel4_recv_with_reply, sel4_reply_recv, sel4_reply_recv_with_reply,
};

pub use executor::sel4_rt_worker_entry as worker_entry;
pub use executor::{
    MAX_TASKS, block_on, poll_one, run, sel4_rt_worker_entry, set_wake_notification, spawn,
    worker_loop,
};
pub use time::{advance, now, sleep_until};
pub use wait::{AsyncLock, AsyncLockGuard, WaitCell};

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
    saved_mrs[..len].copy_from_slice(&mrs[..len]);
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
    YieldNow { yielded: false }
}

pub struct YieldNow {
    yielded: bool,
}

impl Future for YieldNow {
    type Output = ();

    fn poll(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Self::Output> {
        if self.yielded {
            Poll::Ready(())
        } else {
            self.yielded = true;
            cx.waker().wake_by_ref();
            Poll::Pending
        }
    }
}
