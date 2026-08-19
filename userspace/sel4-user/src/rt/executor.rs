use core::future::Future;
use core::mem::{MaybeUninit, align_of, size_of};
use core::pin::Pin;
use core::ptr;
use core::sync::atomic::{AtomicPtr, AtomicU8, AtomicU32, AtomicUsize, Ordering};
use core::task::{Context, Poll, RawWaker, RawWakerVTable, Waker};

use crate::sync::SpinLock;
use crate::{halt_loop, msg_info, sel4_send, sel4_wait, sel4_yield};

pub const MAX_TASKS: usize = 96;
const TASK_SLOT_BYTES: usize = 8192;
const TASK_SLOT_ALIGN: usize = 16;

const SLOT_EMPTY: u8 = 0;
const SLOT_READY: u8 = 1;
const SLOT_RUNNING: u8 = 2;
const SLOT_WAITING: u8 = 3;

type PollFn = unsafe fn(*mut u8, &mut Context<'_>) -> bool;
type DropFn = unsafe fn(*mut u8);

#[repr(align(16))]
struct TaskStorage([MaybeUninit<u8>; TASK_SLOT_BYTES]);

struct TaskSlot {
    state: AtomicU8,
    generation: AtomicU32,
    poll_fn: AtomicPtr<()>,
    drop_fn: AtomicPtr<()>,
    storage: TaskStorage,
}

impl TaskSlot {
    const fn empty() -> Self {
        Self {
            state: AtomicU8::new(SLOT_EMPTY),
            generation: AtomicU32::new(1),
            poll_fn: AtomicPtr::new(ptr::null_mut()),
            drop_fn: AtomicPtr::new(ptr::null_mut()),
            storage: TaskStorage([MaybeUninit::uninit(); TASK_SLOT_BYTES]),
        }
    }

    fn future_ptr(&self) -> *mut u8 {
        self.storage.0.as_ptr() as *mut u8
    }
}

struct ReadyQueue {
    buf: [u64; MAX_TASKS],
    head: usize,
    tail: usize,
    len: usize,
}

impl ReadyQueue {
    const fn new() -> Self {
        Self {
            buf: [0; MAX_TASKS],
            head: 0,
            tail: 0,
            len: 0,
        }
    }
}

static TASKS: [TaskSlot; MAX_TASKS] = [const { TaskSlot::empty() }; MAX_TASKS];
static READY: SpinLock<ReadyQueue> = SpinLock::new(ReadyQueue::new());
static WAKE_NTFN: AtomicUsize = AtomicUsize::new(0);
static PARKED: AtomicUsize = AtomicUsize::new(0);

fn pack_token(slot: usize, generation: u32) -> u64 {
    (generation as u64) << 16 | (slot as u64)
}

fn unpack_token(token: u64) -> (usize, u32) {
    ((token & 0xffff) as usize, (token >> 16) as u32)
}

fn enqueue(slot: usize, generation: u32) {
    let token = pack_token(slot, generation);
    loop {
        {
            let mut ready = READY.lock();
            if ready.len < MAX_TASKS {
                let tail = ready.tail;
                ready.buf[tail] = token;
                ready.tail = (tail + 1) % MAX_TASKS;
                ready.len += 1;
                break;
            }
        }
        unsafe {
            sel4_yield();
        }
    }
    signal_workers();
}

fn dequeue() -> Option<(usize, u32)> {
    let mut ready = READY.lock();
    if ready.len == 0 {
        return None;
    }
    let head = ready.head;
    let token = ready.buf[head];
    ready.head = (head + 1) % MAX_TASKS;
    ready.len -= 1;
    Some(unpack_token(token))
}

fn signal_workers() {
    if PARKED.load(Ordering::SeqCst) == 0 {
        return;
    }
    let ntfn = WAKE_NTFN.load(Ordering::Acquire) as u64;
    if ntfn != 0 {
        unsafe {
            sel4_send(ntfn, msg_info(0, 0, 0, 0), &[]);
        }
    }
}

pub fn set_wake_notification(cptr: u64) {
    WAKE_NTFN.store(cptr as usize, Ordering::Release);
}

pub fn spawn<F>(future: F)
where
    F: Future<Output = ()> + Send + 'static,
{
    const { assert!(size_of::<F>() <= TASK_SLOT_BYTES) };
    const { assert!(align_of::<F>() <= TASK_SLOT_ALIGN) };

    let Some(slot) = alloc_slot() else {
        crate::error!("sel4-user: task arena exhausted");
        halt_loop();
    };
    let task = &TASKS[slot];
    unsafe {
        ptr::write(task.future_ptr() as *mut F, future);
    }
    task.poll_fn
        .store(poll_erased::<F> as *mut (), Ordering::Release);
    task.drop_fn
        .store(drop_erased::<F> as *mut (), Ordering::Release);
    let generation = task.generation.load(Ordering::Relaxed);
    task.state.store(SLOT_READY, Ordering::Release);
    enqueue(slot, generation);
}

fn alloc_slot() -> Option<usize> {
    let mut i = 0usize;
    while i < MAX_TASKS {
        if TASKS[i]
            .state
            .compare_exchange(SLOT_EMPTY, SLOT_READY, Ordering::AcqRel, Ordering::Relaxed)
            .is_ok()
        {
            return Some(i);
        }
        i += 1;
    }
    None
}

unsafe fn poll_erased<F: Future<Output = ()>>(ptr: *mut u8, cx: &mut Context<'_>) -> bool {
    let future = unsafe { &mut *(ptr as *mut F) };
    matches!(
        unsafe { Pin::new_unchecked(future) }.poll(cx),
        Poll::Ready(())
    )
}

unsafe fn drop_erased<F>(ptr: *mut u8) {
    unsafe {
        ptr::drop_in_place(ptr as *mut F);
    }
}

fn waker_for(slot: usize, generation: u32) -> Waker {
    let data = pack_token(slot, generation) as *const ();
    unsafe { Waker::from_raw(RawWaker::new(data, &TASK_WAKER_VTABLE)) }
}

static TASK_WAKER_VTABLE: RawWakerVTable =
    RawWakerVTable::new(waker_clone, waker_wake, waker_wake_by_ref, waker_drop);

fn waker_clone(data: *const ()) -> RawWaker {
    RawWaker::new(data, &TASK_WAKER_VTABLE)
}

fn waker_wake(data: *const ()) {
    waker_wake_by_ref(data);
}

fn waker_wake_by_ref(data: *const ()) {
    let (slot, generation) = unpack_token(data as u64);
    wake_task(slot, generation);
}

fn waker_drop(_: *const ()) {}

fn wake_task(slot: usize, generation: u32) {
    if slot >= MAX_TASKS {
        return;
    }
    let task = &TASKS[slot];
    if task.generation.load(Ordering::Acquire) != generation {
        return;
    }
    loop {
        match task.state.load(Ordering::Acquire) {
            SLOT_WAITING => {
                if task
                    .state
                    .compare_exchange(
                        SLOT_WAITING,
                        SLOT_READY,
                        Ordering::AcqRel,
                        Ordering::Relaxed,
                    )
                    .is_ok()
                {
                    enqueue(slot, generation);
                    return;
                }
            }
            SLOT_RUNNING => {
                if task
                    .state
                    .compare_exchange(
                        SLOT_RUNNING,
                        SLOT_READY,
                        Ordering::AcqRel,
                        Ordering::Relaxed,
                    )
                    .is_ok()
                {
                    return;
                }
            }
            SLOT_READY | SLOT_EMPTY => return,
            _ => return,
        }
    }
}

pub fn poll_one() -> bool {
    let Some((slot, generation)) = dequeue() else {
        return false;
    };
    let task = &TASKS[slot];
    if task.generation.load(Ordering::Acquire) != generation {
        return true;
    }
    match task.state.compare_exchange(
        SLOT_READY,
        SLOT_RUNNING,
        Ordering::AcqRel,
        Ordering::Relaxed,
    ) {
        Ok(_) => {}
        Err(_) => return true,
    }
    let poll_fn = task.poll_fn.load(Ordering::Acquire);
    if poll_fn.is_null() {
        task.state.store(SLOT_EMPTY, Ordering::Release);
        return true;
    }
    let waker = waker_for(slot, generation);
    let mut cx = Context::from_waker(&waker);
    let done =
        unsafe { core::mem::transmute::<*mut (), PollFn>(poll_fn)(task.future_ptr(), &mut cx) };
    if done {
        let drop_fn = task.drop_fn.load(Ordering::Acquire);
        if !drop_fn.is_null() {
            unsafe {
                core::mem::transmute::<*mut (), DropFn>(drop_fn)(task.future_ptr());
            }
        }
        task.poll_fn.store(ptr::null_mut(), Ordering::Release);
        task.drop_fn.store(ptr::null_mut(), Ordering::Release);
        task.generation.fetch_add(1, Ordering::Release);
        task.state.store(SLOT_EMPTY, Ordering::Release);
        return true;
    }
    match task.state.compare_exchange(
        SLOT_RUNNING,
        SLOT_WAITING,
        Ordering::AcqRel,
        Ordering::Relaxed,
    ) {
        Ok(_) => {}
        Err(SLOT_READY) => enqueue(slot, generation),
        Err(_) => {}
    }
    true
}

pub fn run(mut idle: impl FnMut()) -> ! {
    loop {
        while poll_one() {}
        idle();
    }
}

pub fn worker_loop() -> ! {
    let ntfn = WAKE_NTFN.load(Ordering::Acquire) as u64;
    loop {
        if poll_one() {
            continue;
        }
        PARKED.fetch_add(1, Ordering::SeqCst);
        if ready_len() != 0 {
            PARKED.fetch_sub(1, Ordering::SeqCst);
            continue;
        }
        if ntfn != 0 {
            unsafe {
                let _ = sel4_wait(ntfn);
            }
        } else {
            unsafe {
                sel4_yield();
            }
        }
        PARKED.fetch_sub(1, Ordering::SeqCst);
    }
}

fn ready_len() -> usize {
    READY.lock().len
}

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
            Poll::Pending => {
                if !poll_one() {
                    unsafe {
                        sel4_yield();
                    }
                }
            }
        }
    }
}

#[unsafe(no_mangle)]
pub extern "C" fn sel4_rt_worker_entry(ctl: usize) -> ! {
    unsafe {
        crate::install_thread_ctl(ctl as *mut crate::ThreadCtl);
    }
    worker_loop();
}

fn noop_raw_waker() -> RawWaker {
    RawWaker::new(ptr::null(), &NOOP_WAKER_VTABLE)
}

static NOOP_WAKER_VTABLE: RawWakerVTable =
    RawWakerVTable::new(noop_clone, noop_wake, noop_wake_by_ref, noop_drop);

fn noop_clone(_: *const ()) -> RawWaker {
    noop_raw_waker()
}

fn noop_wake(_: *const ()) {}

fn noop_wake_by_ref(_: *const ()) {}

fn noop_drop(_: *const ()) {}
