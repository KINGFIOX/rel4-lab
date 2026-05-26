//! "Current thread" placeholder.
//!
//! In a full implementation this would be a `TCB` with scheduler state,
//! priority, fault endpoint, etc. For M3.1 we only need the bits that
//! the syscall slow path consults: CSpace root, VSpace root, IPC buffer.
//! There is one such thread (the rootserver), so this is just a static
//! singleton.

#![allow(dead_code)]

use core::cell::UnsafeCell;
use core::ptr::null_mut;

use crate::object::cap::Cap;
use crate::object::cnode::Cte;

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

struct Singleton(UnsafeCell<Thread>);
unsafe impl Sync for Singleton {}

static CURRENT: Singleton = Singleton(UnsafeCell::new(Thread::null()));

/// Replace the current-thread state.
///
/// # Safety
/// Single-threaded boot context. Must not be called once user code is
/// running (interrupts off OR caller holds the big kernel lock — we have
/// neither yet but also have one HART and no preemption).
pub unsafe fn set_current(t: Thread) {
    unsafe { *CURRENT.0.get() = t };
}

/// Borrow the current thread record. Returns a `&'static mut` because we
/// only have a single thread; in a real kernel this would be wrapped in
/// a lock or per-CPU pointer.
pub unsafe fn current() -> &'static mut Thread {
    unsafe { &mut *CURRENT.0.get() }
}

/// Helper: install the rootserver thread state. Called once from
/// `bringup_rootserver` after the root CNode is built and BootInfo is
/// laid down.
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
