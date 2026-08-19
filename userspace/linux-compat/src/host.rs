use core::mem::MaybeUninit;

use crate::allocator::Allocator;
use crate::consts::{MAX_PROCS, PROC_UNUSED};
use crate::types::TaskStruct;
use sel4_user::sync::SpinLock;

struct HostState {
    alloc: MaybeUninit<Allocator>,
    ready: bool,
}

static HOST: SpinLock<HostState> = SpinLock::new(HostState {
    alloc: MaybeUninit::uninit(),
    ready: false,
});

pub(crate) fn init(alloc: Allocator) {
    let mut host = HOST.lock();
    host.alloc.write(alloc);
    host.ready = true;
}

pub(crate) fn with_host<R>(f: impl FnOnce(&mut Allocator, &mut [TaskStruct; MAX_PROCS]) -> R) -> R {
    let mut host = HOST.lock();
    if !host.ready {
        crate::util::warn!("linux-compat: host lock used before init");
        crate::util::halt_loop();
    }
    let alloc = unsafe { host.alloc.assume_init_mut() };
    let procs = crate::PROCESS_TABLE.procs();
    f(alloc, procs)
}

pub(crate) fn find_proc(procs: &[TaskStruct; MAX_PROCS], pid: u64) -> Option<usize> {
    let mut i = 0usize;
    while i < MAX_PROCS {
        if procs[i].pid == pid && procs[i].state != PROC_UNUSED {
            return Some(i);
        }
        i += 1;
    }
    None
}
