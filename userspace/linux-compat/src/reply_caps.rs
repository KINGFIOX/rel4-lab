use crate::allocator::Allocator;
use crate::consts::MAX_FAULT_REPLY_CAPS;
use crate::util::{halt_loop, warn};
use sel4_user::sync::SpinLock;
use sel4_user::{msg_info, sel4_cnode_save_caller, sel4_send};

struct ReplyCapPool {
    all: [u64; MAX_FAULT_REPLY_CAPS],
    free: [u64; MAX_FAULT_REPLY_CAPS],
    free_len: usize,
    initialized: bool,
}

static REPLY_CAP_POOL: SpinLock<ReplyCapPool> = SpinLock::new(ReplyCapPool {
    all: [0; MAX_FAULT_REPLY_CAPS],
    free: [0; MAX_FAULT_REPLY_CAPS],
    free_len: 0,
    initialized: false,
});

pub(crate) fn init(alloc: &mut Allocator) {
    let mut pool = REPLY_CAP_POOL.lock();
    if pool.initialized {
        return;
    }
    let mut i = 0usize;
    while i < MAX_FAULT_REPLY_CAPS {
        let slot = alloc.alloc_slot();
        pool.all[i] = slot;
        pool.free[i] = slot;
        i += 1;
    }
    pool.free_len = MAX_FAULT_REPLY_CAPS;
    pool.initialized = true;
}

pub(crate) fn acquire() -> u64 {
    let mut pool = REPLY_CAP_POOL.lock();
    if pool.free_len == 0 {
        warn!("linux-compat: out of reply caps");
        halt_loop();
    }
    pool.free_len -= 1;
    pool.free[pool.free_len]
}

pub(crate) fn save_caller(slot: u64) {
    sel4_cnode_save_caller(slot);
}

pub(crate) fn release(slot: u64) {
    if slot == 0 {
        return;
    }
    let mut pool = REPLY_CAP_POOL.lock();
    if !is_pool_slot(&pool, slot) {
        warn!("linux-compat: attempted to release foreign reply cap");
        halt_loop();
    }
    let mut i = 0usize;
    while i < pool.free_len {
        if pool.free[i] == slot {
            warn!("linux-compat: reply cap released twice");
            halt_loop();
        }
        i += 1;
    }
    if pool.free_len >= MAX_FAULT_REPLY_CAPS {
        warn!("linux-compat: reply cap pool overflow");
        halt_loop();
    }
    let free_len = pool.free_len;
    pool.free[free_len] = slot;
    pool.free_len = free_len + 1;
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

fn is_pool_slot(pool: &ReplyCapPool, slot: u64) -> bool {
    let mut i = 0usize;
    while i < MAX_FAULT_REPLY_CAPS {
        if pool.all[i] == slot {
            return true;
        }
        i += 1;
    }
    false
}
