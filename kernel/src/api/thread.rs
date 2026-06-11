//! "Current thread" view used by the syscall slow path.
//!
//! Historically a static singleton always describing the rootserver.
//! Since M4.2c every thread runs out of a real `Tcb`; this struct is
//! now a per-hart cache refreshed from `tcb::current()` on every context
//! switch (see `refresh_from_tcb` below). Cap lookups and IPC-buffer access
//! via this struct therefore follow whichever TCB the local scheduler picked.

#![allow(dead_code)]

use core::ptr::null_mut;

use crate::object::cap::Cap;
use crate::object::cnode::Cte;
use crate::object::tcb::Tcb;

pub struct Thread {
    /// Pointer to the array of `Cte` that backs the thread's root CNode.
    pub cspace_root: *mut Cte,
    /// log2 number of slots in the root CNode.
    pub cspace_radix: u32,
    /// Bits matched by the root CNode cap's guard.
    pub cspace_guard_bits: u32,
    /// Encoded guard value (only the low `cspace_guard_bits` matter).
    pub cspace_guard: u64,
    /// Kernel-window VA at which the IPC buffer is reachable. We use the
    /// kernel-window mapping rather than the user VA so the syscall path
    /// doesn't have to walk user page tables.
    pub ipc_buffer_kva: *mut u64,
    /// User-mode VA of the IPC buffer (for `SetIPCBuffer` / banner).
    pub ipc_buffer_uva: u64,
    /// Kernel-window VA of the thread's Sv39 root page table (= satp PPN).
    pub vspace_root_kva: u64,
}

impl Thread {
    pub const fn null() -> Self {
        Self {
            cspace_root: null_mut(),
            cspace_radix: 0,
            cspace_guard_bits: 0,
            cspace_guard: 0,
            ipc_buffer_kva: null_mut(),
            ipc_buffer_uva: 0,
            vspace_root_kva: 0,
        }
    }
}

/// Replace the current-thread state.
///
/// # Safety
/// Caller must be running on the target hart with interrupts masked or while
/// holding the kernel lock.
pub unsafe fn set_current(t: Thread) {
    unsafe { crate::kernel::smp::set_current_thread(t) };
}

/// Borrow the current thread record for a scoped operation.
pub unsafe fn with_current<R>(op: impl FnOnce(&mut Thread) -> R) -> R {
    unsafe { crate::kernel::smp::with_current_thread(op) }
}

/// Snapshot the per-hart cached IPC buffer KVA for early/degenerate paths
/// where no current TCB has been published.
fn cached_ipc_buffer_kva_snapshot() -> *mut u64 {
    unsafe { with_current(|thread| thread.ipc_buffer_kva) }
}

pub(crate) fn current_has_ipc_buffer() -> bool {
    let tcb = crate::object::tcb::current();
    if !tcb.is_null() {
        return crate::object::tcb::ipc_buffer_kva_snapshot(tcb) != 0;
    }
    !unsafe { with_current(|thread| thread.ipc_buffer_kva) }.is_null()
}

pub(crate) fn current_ipc_buffer_word(index: usize) -> u64 {
    let tcb = crate::object::tcb::current();
    if !tcb.is_null() {
        return crate::object::tcb::ipc_buffer_word_snapshot(tcb, index);
    }
    let base = cached_ipc_buffer_kva_snapshot();
    if base.is_null() {
        0
    } else {
        unsafe { *base.add(index) }
    }
}

pub(crate) fn write_current_ipc_buffer_word(index: usize, value: u64) -> bool {
    let tcb = crate::object::tcb::current();
    if !tcb.is_null() {
        return unsafe { crate::object::tcb::write_ipc_buffer_word(tcb, index, value) };
    }
    let base = cached_ipc_buffer_kva_snapshot();
    if base.is_null() {
        false
    } else {
        unsafe {
            *base.add(index) = value;
        }
        true
    }
}

pub(crate) fn zero_current_ipc_buffer_words(start: usize, count: usize) {
    let tcb = crate::object::tcb::current();
    if !tcb.is_null() {
        unsafe {
            crate::object::tcb::zero_ipc_buffer_words(tcb, start, count);
        }
        return;
    }
    let base = cached_ipc_buffer_kva_snapshot();
    if base.is_null() {
        return;
    }
    unsafe {
        for i in 0..count {
            *base.add(start + i) = 0;
        }
    }
}

/// Helper: install the rootserver thread state. Called once from
/// `bringup_rootserver` after the root CNode is built and BootInfo is
/// laid down. Later scheduler switches refresh this cache from the
/// selected TCB's seL4-style CTE slots.
pub fn install_rootserver(
    cspace_root: *mut Cte,
    cspace_radix: u32,
    cnode_cap: Cap,
    ipc_buffer_kva: *mut u64,
    ipc_buffer_uva: u64,
    vspace_root_kva: u64,
) {
    let t = Thread {
        cspace_root,
        cspace_radix,
        cspace_guard_bits: cnode_cap.cnode_guard_size() as u32,
        cspace_guard: cnode_cap.cnode_guard(),
        ipc_buffer_kva,
        ipc_buffer_uva,
        vspace_root_kva,
    };
    unsafe { set_current(t) };
}

/// Refresh the static `Thread` from `tcb`'s cap roots. Called by
/// `tcb::set_current` whenever the scheduler swaps the running thread,
/// so every subsequent `thread::with_current()` reads the right CSpace +
/// IPC-buffer for cap lookups inside the syscall path.
///
/// For TCBs whose CTable CTE slot is still null (e.g. a freshly retyped
/// helper before `TCB_Configure` has fired), this is a no-op.
pub unsafe fn refresh_from_tcb(tcb: *const Tcb) {
    if tcb.is_null() {
        return;
    }
    let snapshot = crate::object::tcb::thread_view_snapshot(tcb);
    let cspace_cap = snapshot.cspace_cap;
    if cspace_cap.is_null() {
        return;
    }
    let vspace_cap = snapshot.vspace_cap;
    let radix = cspace_cap.cnode_radix();
    let cnode_ptr = cspace_cap.cnode_ptr();
    let guard_bits = cspace_cap.cnode_guard_size();
    let guard = cspace_cap.cnode_guard();
    let new = Thread {
        cspace_root: cnode_ptr as *mut Cte,
        cspace_radix: radix as u32,
        cspace_guard_bits: guard_bits as u32,
        cspace_guard: guard,
        ipc_buffer_kva: if snapshot.ipc_buffer_kva != 0 {
            snapshot.ipc_buffer_kva as *mut u64
        } else {
            null_mut()
        },
        ipc_buffer_uva: snapshot.ipc_buffer_uva,
        vspace_root_kva: vspace_cap.page_table_base_ptr(),
    };
    unsafe { set_current(new) };
}
