use core::cell::UnsafeCell;

use crate::allocator::Allocator;
use crate::consts::{MAX_FAULT_REPLY_CAPS, OBJ_REPLY};
use crate::util::{halt_loop, warn};
use sel4_user::{msg_info, sel4_send};

struct ReplyCapPool {
    all: UnsafeCell<[u64; MAX_FAULT_REPLY_CAPS]>,
    free: UnsafeCell<[u64; MAX_FAULT_REPLY_CAPS]>,
    free_len: UnsafeCell<usize>,
    current: UnsafeCell<u64>,
    initialized: UnsafeCell<bool>,
}

// xv6-host is a single-threaded rootserver. These cells are only mutated from
// the rootserver fault loop and synchronous helpers it calls.
unsafe impl Sync for ReplyCapPool {}

static REPLY_CAP_POOL: ReplyCapPool = ReplyCapPool {
    all: UnsafeCell::new([0; MAX_FAULT_REPLY_CAPS]),
    free: UnsafeCell::new([0; MAX_FAULT_REPLY_CAPS]),
    free_len: UnsafeCell::new(0),
    current: UnsafeCell::new(0),
    initialized: UnsafeCell::new(false),
};

pub(crate) fn init(alloc: &mut Allocator) {
    unsafe {
        if *REPLY_CAP_POOL.initialized.get() {
            return;
        }
        let all = &mut *REPLY_CAP_POOL.all.get();
        let free = &mut *REPLY_CAP_POOL.free.get();
        let mut i = 0usize;
        while i < MAX_FAULT_REPLY_CAPS {
            let slot = alloc.retype_one(OBJ_REPLY, 0);
            all[i] = slot;
            free[i] = slot;
            i += 1;
        }
        *REPLY_CAP_POOL.free_len.get() = MAX_FAULT_REPLY_CAPS;
        *REPLY_CAP_POOL.initialized.get() = true;
    }
}

pub(crate) fn acquire() -> u64 {
    unsafe {
        let free_len = &mut *REPLY_CAP_POOL.free_len.get();
        if *free_len == 0 {
            warn!("xv6-host: out of reply caps");
            halt_loop();
        }
        *free_len -= 1;
        (&*REPLY_CAP_POOL.free.get())[*free_len]
    }
}

pub(crate) fn set_current(slot: u64) {
    if slot == 0 {
        warn!("xv6-host: attempted to use a null reply cap");
        halt_loop();
    }
    unsafe {
        let current = &mut *REPLY_CAP_POOL.current.get();
        if *current != 0 {
            warn!("xv6-host: reply cap already current");
            halt_loop();
        }
        *current = slot;
    }
}

pub(crate) fn take_current() -> u64 {
    unsafe {
        let current = &mut *REPLY_CAP_POOL.current.get();
        let slot = *current;
        if slot == 0 {
            warn!("xv6-host: no current reply cap");
            halt_loop();
        }
        *current = 0;
        slot
    }
}

pub(crate) fn release_current() {
    let slot = take_current();
    release(slot);
}

pub(crate) fn has_current() -> bool {
    unsafe { *REPLY_CAP_POOL.current.get() != 0 }
}

pub(crate) fn send_and_release(slot: u64, info: u64, mrs: &[u64]) {
    unsafe {
        sel4_send(slot, info, mrs);
    }
    release(slot);
}

pub(crate) fn stop_and_release(slot: u64) {
    send_and_release(slot, msg_info(1, 0, 0, 0), &[]);
}

fn release(slot: u64) {
    if slot == 0 {
        return;
    }
    unsafe {
        if !is_pool_slot(slot) {
            warn!("xv6-host: attempted to release foreign reply cap");
            halt_loop();
        }
        let free_len = &mut *REPLY_CAP_POOL.free_len.get();
        let free = &mut *REPLY_CAP_POOL.free.get();
        let mut i = 0usize;
        while i < *free_len {
            if free[i] == slot {
                warn!("xv6-host: reply cap released twice");
                halt_loop();
            }
            i += 1;
        }
        if *free_len >= MAX_FAULT_REPLY_CAPS {
            warn!("xv6-host: reply cap pool overflow");
            halt_loop();
        }
        free[*free_len] = slot;
        *free_len += 1;
    }
}

unsafe fn is_pool_slot(slot: u64) -> bool {
    let all = unsafe { &*REPLY_CAP_POOL.all.get() };
    let mut i = 0usize;
    while i < MAX_FAULT_REPLY_CAPS {
        if all[i] == slot {
            return true;
        }
        i += 1;
    }
    false
}
