//! Kernel-side TCB object.
//!
//! Lives in the 2 KiB (`seL4_TCBBits = 11`) region the user retypes from
//! an Untyped via `Untyped_Retype(seL4_TCBObject)`. Because that region
//! is always `2 KiB`-aligned and bigger than `size_of::<Tcb>()`, the cap's
//! pointer is a valid [`TcbRef`].
//!
//! The scheduler is deliberately small: runnable TCBs live in one FIFO
//! round-robin queue per core, affinity selects the queue a TCB can run on,
//! and a per-TCB timeslice decides when a still-runnable thread is rotated.
//! Each core also has a static idle TCB, matching seL4 `ksIdleThread`. Idle is
//! never enqueued; an empty runqueue switches `current` to that core's idle
//! TCB and waits in kernel mode. A temporary big kernel lock still serialises
//! most shared kernel state while the SMP path matures.
//!
//! Layout-load: every field must fit comfortably inside the 2 KiB slab
//! the C kernel allocates, so the future C/Rust ABI swap stays valid. A
//! freshly retyped slab is all zeroes, but this module does not lean on that
//! matching each field's resting encoding: [`init`] writes a complete
//! [`Tcb`] over the slab before anything reads it.
//!
//! All access goes through [`TcbRef`], so this module needs no raw pointer
//! dereferences of its own. One caveat comes with the CTEs embedded in a TCB:
//! [`ObjRef::with_mut`] on the TCB and a [`CteRef`] into the same TCB would
//! alias, so code holds one or the other, never both at once.

#![allow(dead_code)]

use core::sync::atomic::{AtomicBool, AtomicUsize, Ordering};

use log_crate::info;

use crate::abi::constants::{MAX_NUM_NODES, TIME_SLICE_TICKS};
use crate::arch::current::api::UserContext;
use crate::arch::current::sel4_arch;
use crate::kernel::smp::BklCell;
use crate::ktypes::addr::UserVa;
use crate::ktypes::list::{self, Linked, Links, Queue, QueueEnds};
use crate::ktypes::objref::{ObjArray, ObjCell, ObjRef};
use crate::object::cap::Cap;
use crate::object::cnode::{Cte, CteRef};
use crate::object::endpoint::EndpointRef;
use crate::object::notification::NotificationRef;

/// Handle for a thread control block.
pub type TcbRef = ObjRef<Tcb>;

/// The currently-scheduled TCB for the local core, if any.
#[inline]
pub fn current() -> Option<TcbRef> {
    crate::kernel::smp::current_tcb()
}

/// Replace the local core's current TCB, returning the previous one.
#[inline]
pub fn set_current(tcb: Option<TcbRef>) -> Option<TcbRef> {
    crate::kernel::smp::set_current_tcb(tcb)
}

const DEFAULT_TIME_SLICE: u8 = TIME_SLICE_TICKS as u8;

const _: () = {
    assert!(TIME_SLICE_TICKS > 0 && TIME_SLICE_TICKS <= u8::MAX as usize);
};

static CONTINUE_CURRENT_ONCE: [AtomicUsize; MAX_NUM_NODES] =
    [const { AtomicUsize::new(0) }; MAX_NUM_NODES];
static RESCHEDULE_REQUIRED: [AtomicBool; MAX_NUM_NODES] =
    [const { AtomicBool::new(false) }; MAX_NUM_NODES];

pub(crate) fn continue_current_once(tcb: TcbRef) {
    let core = crate::kernel::smp::current_core_id();
    CONTINUE_CURRENT_ONCE[core].store(tcb.kva() as usize, Ordering::Release);
}

pub(crate) fn take_continue_current_once(tcb: Option<TcbRef>) -> bool {
    let Some(tcb) = tcb else {
        return false;
    };
    let core = crate::kernel::smp::current_core_id();
    CONTINUE_CURRENT_ONCE[core]
        .compare_exchange(tcb.kva() as usize, 0, Ordering::AcqRel, Ordering::Acquire)
        .is_ok()
}

fn request_reschedule() {
    let core = crate::kernel::smp::current_core_id();
    RESCHEDULE_REQUIRED[core].store(true, Ordering::Release);
}

/// Consume the per-core timeslice/Yield rotation request, if any.
pub fn take_reschedule_required() -> bool {
    let core = crate::kernel::smp::current_core_id();
    RESCHEDULE_REQUIRED[core].swap(false, Ordering::AcqRel)
}

/// seL4-style `timerTick`: decrement the current thread's timeslice, and
/// request a rotation only when it reaches zero.
pub fn timer_tick(current: TcbRef) {
    if is_idle_thread(current) {
        return;
    }
    let expired = current.with_mut(|t| {
        if t.state != ThreadState::Running {
            return false;
        }
        if t.time_slice > 1 {
            t.time_slice -= 1;
            false
        } else {
            t.time_slice = DEFAULT_TIME_SLICE;
            true
        }
    });
    if expired {
        request_reschedule();
    }
}

/// Explicit `Yield`: put the caller on the runqueue tail and rotate at
/// the next `kernel_exit`.
pub fn yield_current(current: TcbRef) {
    if is_idle_thread(current) {
        return;
    }
    current.enqueue();
    request_reschedule();
}

/// Per-core idle TCBs. Matches seL4 `ksIdleThreadTCB[CONFIG_MAX_NUM_NODES]`.
static IDLE_TCBS: [ObjCell<Tcb>; MAX_NUM_NODES] =
    [const { ObjCell::new(Tcb::zero()) }; MAX_NUM_NODES];

/// Idle TCB for `core`, or `None` if `core` is out of range.
#[inline]
pub fn idle_thread_of(core: usize) -> Option<TcbRef> {
    IDLE_TCBS.get(core).map(ObjCell::get)
}

/// True if `tcb` is one of the static per-core idle threads.
#[inline]
pub fn is_idle_thread(tcb: TcbRef) -> bool {
    (0..MAX_NUM_NODES).any(|core| idle_thread_of(core) == Some(tcb))
}

/// Make this core's idle TCB current. Idle is never placed on a runqueue and
/// must not be restored through `sret`/`sysret`; the trap path waits in
/// kernel mode instead.
pub fn switch_to_idle_thread() -> TcbRef {
    let idle = idle_thread_of(crate::kernel::smp::current_core_id())
        .expect("idle thread missing for current core");
    set_current(Some(idle));
    idle
}

/// Initialise every core's idle TCB. Called once on the boot core before
/// application processors start and before the rootserver runs.
pub fn create_idle_threads() {
    for core in 0..MAX_NUM_NODES {
        let idle = idle_thread_of(core).expect("idle thread missing");
        let kernel_sp = crate::kernel::smp::kernel_stack_top_for_core(core) as u64;
        idle.with_mut(|t| {
            t.state = ThreadState::Idle;
            t.affinity = core as u8;
            t.time_slice = DEFAULT_TIME_SLICE;
            t.flags = TCB_FLAG_FPU_DISABLED;
            let name = b"idle_thread";
            t.name[..name.len()].copy_from_slice(name);
            sel4_arch::configure_idle_context(&mut t.context, kernel_sp);
        });
    }
    info!("microkernel: created {} idle thread(s)", MAX_NUM_NODES);
}

// ---- Ready-queue runqueues ------------------------------------------------
//
// One doubly-linked FIFO list per core, backed by the `Links` embedded in each
// `Tcb`. Endpoint and notification wait lists reuse those same links, which is
// sound because a TCB is either runnable (in a runqueue) or blocked on a wait
// object, never both.
//
// User execution can happen on more than one core. Queue links and the
// surrounding TCB state transitions are serialized by the seL4-style big
// kernel lock.

pub const TCB_CNODE_RADIX: usize = 4;
pub const TCB_CNODE_ENTRIES: usize = 1 << TCB_CNODE_RADIX;
pub const TCB_ARCH_CNODE_ENTRIES: usize = TCB_CNODE_ENTRIES;
pub const TCB_CTABLE_SLOT: usize = 0;
pub const TCB_VTABLE_SLOT: usize = 1;
pub const TCB_REPLY: usize = 2;
pub const TCB_CALLER: usize = 3;
pub const TCB_BUFFER_SLOT: usize = 4;
pub(crate) const TCB_SENDER_EXTRA_CAPS: usize = 3;
pub const TCB_FLAG_FPU_DISABLED: u64 = 0x1;
pub const TCB_FLAG_MASK: u64 = TCB_FLAG_FPU_DISABLED;

static RUNQUEUES: [BklCell<Queue<Tcb>>; MAX_NUM_NODES] =
    [const { BklCell::new(Queue::new()) }; MAX_NUM_NODES];

#[inline]
fn core_for_affinity(affinity: u8) -> usize {
    let core = affinity as usize;
    if core < MAX_NUM_NODES { core } else { 0 }
}

/// Number of words in a thread's IPC buffer (`seL4_IPCBufferSizeBits = 10`).
pub const IPC_BUFFER_WORDS: usize = 128;

/// A thread's IPC buffer, addressed through the kernel window rather than
/// through the user mapping, so the syscall path does not have to walk user
/// page tables.
#[repr(transparent)]
#[derive(Copy, Clone, PartialEq, Eq, Debug)]
pub struct IpcBuffer(ObjRef<u64>);

impl IpcBuffer {
    /// The buffer mapped at kernel-window address `kva`.
    ///
    /// # Safety
    /// `kva` must be the base of a frame mapped into the kernel window and
    /// large enough for [`IPC_BUFFER_WORDS`] words.
    #[inline]
    pub const unsafe fn from_kva(kva: u64) -> Option<Self> {
        // SAFETY: forwarded to the caller.
        match unsafe { ObjRef::from_kva(kva) } {
            Some(base) => Some(Self(base)),
            None => None,
        }
    }

    #[inline]
    pub fn kva(self) -> u64 {
        self.0.kva()
    }

    #[inline]
    fn words(self) -> ObjArray<u64> {
        // SAFETY: the constructor promised a frame with at least this many
        // words, contiguous because it is one mapped frame.
        unsafe { ObjArray::new(self.0, IPC_BUFFER_WORDS) }
    }

    /// Read one word, or zero when the index is outside the buffer.
    #[inline]
    pub fn word(self, index: usize) -> u64 {
        self.words()
            .with_slice(|words| words.get(index).copied().unwrap_or(0))
    }

    /// Write one word, reporting whether it was in range.
    #[inline]
    pub fn set_word(self, index: usize, value: u64) -> bool {
        self.words()
            .with_slice_mut(|words| match words.get_mut(index) {
                Some(slot) => {
                    *slot = value;
                    true
                }
                None => false,
            })
    }

    /// Write consecutive words starting at `start`.
    pub fn set_words(self, start: usize, values: &[u64]) -> bool {
        self.words().with_slice_mut(|words| {
            let Some(dst) = words.get_mut(start..start + values.len()) else {
                return false;
            };
            dst.copy_from_slice(values);
            true
        })
    }

    /// Zero `count` words starting at `start`.
    pub fn zero_words(self, start: usize, count: usize) -> bool {
        self.words().with_slice_mut(|words| {
            let Some(dst) = words.get_mut(start..start + count) else {
                return false;
            };
            dst.fill(0);
            true
        })
    }

    /// Copy `count` words at `start` out of `src` into this buffer.
    pub fn copy_words_from(self, src: IpcBuffer, start: usize, count: usize) -> bool {
        if self == src {
            // Two threads may share one IPC buffer frame; copying a range onto
            // itself is a no-op, and borrowing it twice would alias.
            return start + count <= IPC_BUFFER_WORDS;
        }
        let mut staged = [0u64; IPC_BUFFER_WORDS];
        let read = src
            .words()
            .with_slice(|words| match words.get(start..start + count) {
                Some(src) => {
                    staged[..count].copy_from_slice(src);
                    true
                }
                None => false,
            });
        read && self.set_words(start, &staged[..count])
    }
}

/// The object a blocked thread is queued on. Which kind it is follows from
/// the thread state, but naming it here keeps the dispatch honest.
#[derive(Copy, Clone, PartialEq, Eq, Debug)]
pub enum WaitObject {
    Endpoint(EndpointRef),
    Notification(NotificationRef),
}

impl WaitObject {
    #[inline]
    pub fn endpoint(self) -> Option<EndpointRef> {
        match self {
            WaitObject::Endpoint(ep) => Some(ep),
            WaitObject::Notification(_) => None,
        }
    }

    #[inline]
    pub fn notification(self) -> Option<NotificationRef> {
        match self {
            WaitObject::Notification(ntfn) => Some(ntfn),
            WaitObject::Endpoint(_) => None,
        }
    }
}

/// State of a thread that is blocked waiting to receive.
#[derive(Copy, Clone, PartialEq, Eq, Debug)]
pub enum BlockedReceive {
    /// Queued on an endpoint's receive list.
    OnEndpoint(EndpointRef),
    /// Marked blocked but not on a queue, i.e. mid-rendezvous.
    Detached,
}

pub(crate) const FAULT_IPC_MRS: usize = 20;
pub(crate) type FaultMrs = [u64; FAULT_IPC_MRS];

#[derive(Copy, Clone)]
pub(crate) struct FaultIpcMessage {
    pub label: u64,
    pub len: u64,
    pub mrs: FaultMrs,
}

#[derive(Copy, Clone)]
pub(crate) struct QueuedSenderSnapshot {
    pub info_word: u64,
    pub badge: u64,
    pub is_call: bool,
    pub can_grant: bool,
    pub can_grant_reply: bool,
    pub extra_cap_slots: [Option<CteRef>; TCB_SENDER_EXTRA_CAPS],
    pub is_fault: bool,
    pub fault_label: u64,
}

#[derive(Copy, Clone)]
pub(crate) struct ThreadViewSnapshot {
    pub cspace_cap: Cap,
    pub vspace_cap: Cap,
    pub ipc_buffer: Option<IpcBuffer>,
    pub ipc_buffer_uva: UserVa,
}

pub const TCB_NAME_LEN: usize = 32;

/// Mirrors `_thread_state` in `kernel/include/object/structures.h`. The
/// numbering doesn't need to match upstream because we never expose this
/// over the ABI; we only use it for our own scheduler.
#[repr(u8)]
#[derive(Copy, Clone, PartialEq, Eq, Debug, Default)]
pub enum ThreadState {
    #[default]
    Inactive = 0,
    Restart = 1,
    Running = 2,
    BlockedOnReceive = 3,
    BlockedOnSend = 4,
    BlockedOnReply = 5,
    BlockedOnNotification = 6,
    Idle = 7,
}

impl ThreadState {
    #[inline]
    pub fn is_runnable(self) -> bool {
        matches!(self, ThreadState::Running | ThreadState::Restart)
    }

    /// States that `Resume` will restart from, mirroring seL4's set of
    /// "stopped" states.
    #[inline]
    fn is_stopped(self) -> bool {
        matches!(
            self,
            ThreadState::Inactive
                | ThreadState::BlockedOnReceive
                | ThreadState::BlockedOnSend
                | ThreadState::BlockedOnNotification
                | ThreadState::BlockedOnReply
        )
    }
}

#[repr(C, align(32))]
pub struct Tcb {
    /// seL4-style CTEs embedded in the TCB object. Slots 0..4 are CSpace,
    /// VSpace, master Reply, Caller, and IPC buffer. The remaining slots
    /// exist because ZombieTCB uses radix 4.
    pub ctes: [Cte; TCB_CNODE_ENTRIES],

    /// Saved user-mode register state. The trap path restores this on
    /// `sret` once a scheduler picks the TCB.
    pub context: UserContext,

    /// Unprioritised round-robin scheduling state.
    state: ThreadState,
    affinity: u8,
    time_slice: u8,

    /// User-mode VA at which the IPC buffer is mapped.
    ipc_buffer_uva: UserVa,
    /// The same buffer as reached through the kernel window. Lazily
    /// resolved; `None` means "not yet set up".
    ipc_buffer: Option<IpcBuffer>,

    /// Fault handler CPtr, resolved in the target thread's current CSpace
    /// when a fault is delivered.
    fault_endpoint_cptr: u64,

    /// seL4_TCBFlag bits. RISC-V currently uses bit 0 for fpuDisabled.
    flags: u64,

    /// The bound `Notification`, if any.
    bound_notification: Option<NotificationRef>,

    /// Runqueue and wait-list links, owned by `ktypes::list`.
    queue: Links<Tcb>,

    /// Object the TCB is currently blocked on, used to dequeue on
    /// cancel / destroy.
    waiting_on: Option<WaitObject>,
    receiver_can_grant: bool,

    /// Badge from the cap used to Send / Call. Stashed when a sender
    /// blocks on an Endpoint so the eventual receiver can read it back
    /// without re-walking the sender's CSpace.
    sender_badge: u64,
    sender_extra_cap_slots: [Option<CteRef>; TCB_SENDER_EXTRA_CAPS],
    sender_can_grant: bool,
    sender_can_grant_reply: bool,
    /// Set iff the queued-up Send was originally a `seL4_Call`. The
    /// receiver consults this on rendezvous to decide whether to put
    /// the sender into `BlockedOnReply` (Call) or wake it directly
    /// (plain Send).
    sender_is_call: bool,

    /// Set iff the queued-up Call is a synthetic fault IPC. Fault IPC
    /// must not borrow the faulting thread's message registers: the
    /// handler reply restarts the trapped instruction, so the original
    /// user register file has to survive intact.
    sender_is_fault: bool,
    fault_label: u64,
    fault_len: u64,
    fault_mrs: FaultMrs,

    /// Debug name, populated by `seL4_DebugNameThread`. NUL-padded.
    name: [u8; TCB_NAME_LEN],
}

// SAFETY: `queue` is a dedicated field with no other accessor, so the queue
// operations in `ktypes::list` are its only writer.
unsafe impl Linked for Tcb {
    fn links(this: &mut Self) -> &mut Links<Self> {
        &mut this.queue
    }
}

// Compile-time sanity: must fit inside the 2 KiB Untyped slab (= 2048
// bytes for SEL4_TCB_BITS = 11), including the embedded TCB CTEs.
const _: () = {
    assert!(size_of::<Tcb>() <= 2048);
};

impl Tcb {
    /// All-zero TCB constructor for static / BSS use (the rootserver TCB
    /// is created this way; user-allocated TCBs go through `init` after
    /// `Untyped_Retype` zeroes their slab).
    pub const fn zero() -> Self {
        Tcb {
            ctes: [Cte::null(); TCB_CNODE_ENTRIES],
            context: UserContext::zero(),
            state: ThreadState::Inactive,
            affinity: 0,
            time_slice: DEFAULT_TIME_SLICE,
            ipc_buffer_uva: UserVa::ZERO,
            ipc_buffer: None,
            fault_endpoint_cptr: 0,
            flags: 0,
            bound_notification: None,
            queue: Links::unlinked(),
            waiting_on: None,
            receiver_can_grant: false,
            sender_badge: 0,
            sender_extra_cap_slots: [None; TCB_SENDER_EXTRA_CAPS],
            sender_can_grant: false,
            sender_can_grant_reply: false,
            sender_is_call: false,
            sender_is_fault: false,
            fault_label: 0,
            fault_len: 0,
            fault_mrs: [0; FAULT_IPC_MRS],
            name: [0; TCB_NAME_LEN],
        }
    }

    /// Forget stale endpoint send/receive metadata.
    fn clear_endpoint_ipc_state(&mut self) {
        self.receiver_can_grant = false;
        self.sender_badge = 0;
        self.sender_can_grant = false;
        self.sender_can_grant_reply = false;
        self.sender_extra_cap_slots = [None; TCB_SENDER_EXTRA_CAPS];
        self.sender_is_call = false;
        self.clear_fault();
    }

    /// As `clear_endpoint_ipc_state`, but leave a pending fault in place for
    /// the reply path to consume. Explicit `cancel_ipc` still clears it.
    fn clear_endpoint_ipc_state_preserving_fault(&mut self) {
        let fault = (
            self.sender_is_fault,
            self.fault_label,
            self.fault_len,
            self.fault_mrs,
        );
        self.clear_endpoint_ipc_state();
        if fault.0 {
            self.sender_is_fault = true;
            self.fault_label = fault.1;
            self.fault_len = fault.2;
            self.fault_mrs = fault.3;
        }
    }

    fn clear_fault(&mut self) {
        self.sender_is_fault = false;
        self.fault_label = 0;
        self.fault_len = 0;
        self.fault_mrs = [0; FAULT_IPC_MRS];
    }

    fn rewind_pc(&mut self) {
        self.context.pc = self.context.pc.wrapping_sub(4);
        self.context.restart_pc = self.context.pc;
    }

    fn write_notification_badge_regs(&mut self, badge: u64) {
        self.context.set_cap_reg(badge);
        self.context.set_msg_info(0);
        for i in 0..4 {
            self.context.set_mr(i, 0);
        }
    }

    #[inline]
    fn cap_at(&self, index: usize) -> Cap {
        self.ctes.get(index).map_or(Cap::null(), |cte| cte.cap)
    }
}

/// The scoped-access API the rest of the kernel uses. Every method takes the
/// big kernel lock's word for it that no other core is looking at this TCB.
impl TcbRef {
    // ---- plain state ----

    #[inline]
    pub fn state(self) -> ThreadState {
        self.with(|t| t.state)
    }

    #[inline]
    pub fn set_state(self, state: ThreadState) {
        self.with_mut(|t| t.state = state);
    }

    #[inline]
    pub fn is_runnable(self) -> bool {
        self.state().is_runnable()
    }

    #[inline]
    pub fn affinity(self) -> u8 {
        self.with(|t| t.affinity)
    }

    #[inline]
    pub fn home_core(self) -> usize {
        core_for_affinity(self.affinity())
    }

    #[inline]
    pub fn flags(self) -> u64 {
        self.with(|t| t.flags)
    }

    #[inline]
    pub fn fpu_disabled(self) -> bool {
        self.flags() & TCB_FLAG_FPU_DISABLED != 0
    }

    #[inline]
    pub fn fault_endpoint_cptr(self) -> u64 {
        self.with(|t| t.fault_endpoint_cptr)
    }

    #[inline]
    pub fn set_fault_endpoint_cptr(self, cptr: u64) {
        self.with_mut(|t| t.fault_endpoint_cptr = cptr);
    }

    #[inline]
    pub fn waiting_on(self) -> Option<WaitObject> {
        self.with(|t| t.waiting_on)
    }

    #[inline]
    pub fn clear_waiting_on(self) {
        self.with_mut(|t| t.waiting_on = None);
    }

    #[inline]
    pub fn bound_notification(self) -> Option<NotificationRef> {
        self.with(|t| t.bound_notification)
    }

    #[inline]
    pub fn sender_badge(self) -> u64 {
        self.with(|t| t.sender_badge)
    }

    #[inline]
    pub fn receiver_can_grant(self) -> bool {
        self.with(|t| t.receiver_can_grant)
    }

    #[inline]
    pub fn ipc_buffer(self) -> Option<IpcBuffer> {
        self.with(|t| t.ipc_buffer)
    }

    #[inline]
    pub fn has_ipc_buffer(self) -> bool {
        self.ipc_buffer().is_some()
    }

    #[inline]
    pub fn ipc_buffer_uva(self) -> UserVa {
        self.with(|t| t.ipc_buffer_uva)
    }

    #[inline]
    pub fn cspace_cap(self) -> Cap {
        self.with(|t| t.cap_at(TCB_CTABLE_SLOT))
    }

    #[inline]
    pub fn vspace_cap(self) -> Cap {
        self.with(|t| t.cap_at(TCB_VTABLE_SLOT))
    }

    /// CSpace root, VSpace root, and IPC buffer in one borrow, for the
    /// syscall path's per-switch refresh.
    pub fn thread_view(self) -> ThreadViewSnapshot {
        self.with(|t| ThreadViewSnapshot {
            cspace_cap: t.cap_at(TCB_CTABLE_SLOT),
            vspace_cap: t.cap_at(TCB_VTABLE_SLOT),
            ipc_buffer: t.ipc_buffer,
            ipc_buffer_uva: t.ipc_buffer_uva,
        })
    }

    /// One of the CTEs embedded in this TCB.
    ///
    /// The handle points inside the TCB, so it must not be used while a
    /// borrow of the TCB itself is live.
    #[inline]
    pub fn cap_slot(self, index: usize) -> Option<CteRef> {
        crate::kernel::smp::debug_assert_kernel_lock_held();
        self.ctes().get(index)
    }

    /// All CTEs embedded in this TCB, as the CNode that `ZombieTCB` deletes.
    #[inline]
    pub fn ctes(self) -> ObjArray<Cte> {
        // SAFETY: `Tcb::ctes` is a contiguous array of exactly this length
        // inside the live object this handle addresses.
        let base = unsafe { ObjRef::from_kva_unchecked(self.kva()) };
        // SAFETY: `ctes` is the first field of a `repr(C)` `Tcb`, so the
        // object's base address is also the array's base address.
        unsafe { ObjArray::new(base, TCB_CNODE_ENTRIES) }
    }

    /// Borrow the saved user context, for the FPU paths that copy hardware
    /// register state in and out of it.
    #[inline]
    pub fn with_context<R>(self, op: impl FnOnce(&UserContext) -> R) -> R {
        self.with(|t| op(&t.context))
    }

    /// Borrow the saved user context mutably.
    #[inline]
    pub fn with_context_mut<R>(self, op: impl FnOnce(&mut UserContext) -> R) -> R {
        self.with_mut(|t| op(&mut t.context))
    }

    /// Address of the saved user context, for the assembly restore path.
    ///
    /// Derived from the object's address rather than from a borrow, because
    /// the pointer outlives this call: the trap exit path hands it to
    /// assembly, and nothing dereferences it from Rust.
    #[inline]
    pub fn context_ptr(self) -> *mut UserContext {
        // SAFETY: projecting a field of a live object without forming a
        // reference to the object itself.
        unsafe { &raw mut (*self.as_ptr()).context }
    }

    // ---- blocked-state queries ----

    #[inline]
    pub fn blocked_on_reply(self) -> bool {
        self.state() == ThreadState::BlockedOnReply
    }

    #[inline]
    pub fn blocked_on_receive(self) -> bool {
        self.state() == ThreadState::BlockedOnReceive
    }

    /// Receive-blocked state, distinguishing "queued on an endpoint" from
    /// "detached mid-rendezvous".
    pub fn blocked_receive(self) -> Option<BlockedReceive> {
        self.with(|t| {
            if t.state != ThreadState::BlockedOnReceive {
                return None;
            }
            Some(match t.waiting_on.and_then(WaitObject::endpoint) {
                Some(ep) => BlockedReceive::OnEndpoint(ep),
                None => BlockedReceive::Detached,
            })
        })
    }

    /// Whether this thread is queued on `endpoint` in the given direction.
    pub fn waits_on_endpoint(self, endpoint: EndpointRef, sending: bool) -> bool {
        let expected = if sending {
            ThreadState::BlockedOnSend
        } else {
            ThreadState::BlockedOnReceive
        };
        self.with(|t| t.state == expected && t.waiting_on == Some(WaitObject::Endpoint(endpoint)))
    }

    /// Whether this thread is queued on `notification`.
    pub fn waits_on_notification(self, notification: NotificationRef) -> bool {
        self.with(|t| {
            t.state == ThreadState::BlockedOnNotification
                && t.waiting_on == Some(WaitObject::Notification(notification))
        })
    }

    // ---- IPC payload accessors ----

    pub fn ipc_message_regs(self, length: u64) -> [u64; 4] {
        let mut regs = [0u64; 4];
        self.with(|t| {
            for (i, reg) in regs.iter_mut().enumerate().take(length.min(4) as usize) {
                *reg = t.context.mr(i);
            }
        });
        regs
    }

    pub fn write_ipc_message_regs(self, badge: u64, mr_regs: &[u64; 4], length: u64) {
        self.with_mut(|t| {
            t.context.set_cap_reg(badge);
            for i in 0..length.min(4) as usize {
                t.context.set_mr(i, mr_regs[i]);
            }
        });
    }

    pub fn write_fault_ipc_message_regs(
        self,
        badge: u64,
        info_word: u64,
        mrs: &[u64],
        length: u64,
    ) {
        self.with_mut(|t| {
            t.context.set_cap_reg(badge);
            t.context.set_msg_info(info_word);
            for i in 0..length.min(4).min(mrs.len() as u64) as usize {
                t.context.set_mr(i, mrs[i]);
            }
        });
    }

    pub fn write_message_info(self, info_word: u64) {
        self.with_mut(|t| t.context.set_msg_info(info_word));
    }

    pub fn write_user_context(self, pc: Option<u64>, regs: &[(usize, u64)]) {
        self.with_mut(|t| {
            if let Some(pc) = pc {
                sel4_arch::apply_written_pc(&mut t.context, pc);
            }
            for &(idx, value) in regs {
                if let Some(slot) = t.context.regs.get_mut(idx) {
                    *slot = value;
                }
            }
        });
    }

    pub fn user_context_word(self, context_index: usize, reg_index: usize) -> u64 {
        self.with(|t| match context_index {
            0 => t.context.restart_pc,
            #[cfg(target_arch = "riscv64")]
            1 => t.context.return_reg(),
            _ => t.context.regs.get(reg_index).copied().unwrap_or(0),
        })
    }

    pub fn set_tls_base(self, tls_base: u64) {
        self.with_mut(|t| t.context.set_tls_reg(tls_base));
    }

    /// Pin this thread to `core`. Only meaningful before it is queued.
    pub fn set_initial_affinity(self, core: u8) {
        self.with_mut(|t| t.affinity = core);
    }

    pub fn set_fpu_context_enabled(self, enabled: bool) {
        self.with_mut(|t| sel4_arch::set_fpu_context_enabled(&mut t.context, enabled));
    }

    pub fn queued_sender(self) -> QueuedSenderSnapshot {
        self.with(|t| QueuedSenderSnapshot {
            info_word: t.context.msg_info(),
            badge: t.sender_badge,
            is_call: t.sender_is_call,
            can_grant: t.sender_can_grant,
            can_grant_reply: t.sender_can_grant_reply,
            extra_cap_slots: t.sender_extra_cap_slots,
            is_fault: t.sender_is_fault,
            fault_label: if t.sender_is_fault { t.fault_label } else { 0 },
        })
    }

    pub fn sender_fault(self) -> (bool, u64) {
        self.with(|t| {
            (
                t.sender_is_fault,
                if t.sender_is_fault { t.fault_label } else { 0 },
            )
        })
    }

    pub fn fault_message(self) -> FaultIpcMessage {
        self.with(|t| FaultIpcMessage {
            label: t.fault_label,
            len: t.fault_len,
            mrs: t.fault_mrs,
        })
    }

    pub fn record_fault_message(self, label: u64, len: u64, mrs: FaultMrs) {
        self.with_mut(|t| {
            t.sender_is_fault = true;
            t.fault_label = label;
            t.fault_len = len;
            t.fault_mrs = mrs;
        });
    }

    pub fn clear_fault_message(self) {
        self.with_mut(Tcb::clear_fault);
    }

    // ---- state transitions ----

    pub fn set_inactive(self) {
        self.set_state(ThreadState::Inactive);
    }

    pub fn rewind_pc(self) {
        self.with_mut(Tcb::rewind_pc);
    }

    pub fn clear_blocked_receive_state(self) {
        self.with_mut(|t| {
            t.waiting_on = None;
            t.clear_endpoint_ipc_state();
        });
    }

    pub fn set_blocked_on_reply(self) {
        self.with_mut(|t| {
            t.waiting_on = None;
            t.clear_endpoint_ipc_state_preserving_fault();
            t.state = ThreadState::BlockedOnReply;
        });
    }

    /// Mark a selected runnable TCB active and return the saved user context
    /// that the trap restore path should load.
    pub fn prepare_for_user_restore(self) -> *mut UserContext {
        self.with_mut(|t| {
            if t.state == ThreadState::Restart {
                t.context.pc = t.context.restart_pc;
                t.state = ThreadState::Running;
            }
        });
        crate::arch::current::machine::fpu::lazy_restore(self);
        self.context_ptr()
    }

    pub fn set_running_with_reply_regs(self, badge: u64, info_word: u64) {
        self.with_mut(|t| {
            t.context.set_cap_reg(badge);
            t.context.set_msg_info(info_word);
            t.state = ThreadState::Running;
        });
    }

    pub fn set_blocked_on_notification(self, notification: NotificationRef) {
        self.with_mut(|t| {
            t.state = ThreadState::BlockedOnNotification;
            t.waiting_on = Some(WaitObject::Notification(notification));
        });
    }

    pub fn complete_notification_wait(self, badge: u64) {
        self.with_mut(|t| {
            t.waiting_on = None;
            t.write_notification_badge_regs(badge);
            t.state = ThreadState::Running;
        });
    }

    pub fn complete_bound_endpoint_notification_wait(self, badge: u64) {
        self.with_mut(|t| {
            t.waiting_on = None;
            t.clear_endpoint_ipc_state();
            t.write_notification_badge_regs(badge);
            t.state = ThreadState::Running;
        });
    }

    pub fn cancel_ipc(self) {
        if self.blocked_on_reply() {
            crate::object::reply::cancel_blocked_on_reply(self);
        }
        self.with_mut(|t| {
            t.waiting_on = None;
            t.clear_endpoint_ipc_state();
            t.state = ThreadState::Inactive;
        });
    }

    /// Unlink from a detached endpoint wait list and report the next waiter
    /// plus whether this one should be made runnable.
    pub fn cancel_endpoint_waiter(self, call_error_info: Option<u64>) -> (Option<TcbRef>, bool) {
        let next = list::next_of(self);
        self.with_mut(|t| {
            if t.state != ThreadState::BlockedOnSend && t.state != ThreadState::BlockedOnReceive {
                t.queue = Links::unlinked();
                t.waiting_on = None;
                t.clear_endpoint_ipc_state();
                return (next, false);
            }
            let was_call = t.sender_is_call;
            let was_fault_sender = t.state == ThreadState::BlockedOnSend && t.sender_is_fault;
            let preserve_fault = was_fault_sender && call_error_info.is_none();

            t.queue = Links::unlinked();
            t.waiting_on = None;
            if preserve_fault {
                t.clear_endpoint_ipc_state_preserving_fault();
            } else {
                t.clear_endpoint_ipc_state();
            }

            if let Some(info_word) = call_error_info {
                t.context.set_cap_reg(0);
                if was_call {
                    t.context.set_msg_info(info_word);
                    for i in 0..4 {
                        t.context.set_mr(i, 0);
                    }
                }
            }

            if was_fault_sender {
                t.state = ThreadState::Inactive;
                (next, false)
            } else {
                t.state = ThreadState::Restart;
                (next, true)
            }
        })
    }

    pub fn restart_endpoint_waiter(self) -> Option<TcbRef> {
        self.cancel_endpoint_waiter(None).0
    }

    pub fn cancel_badged_sender(self, call_error_info: u64) -> Option<TcbRef> {
        self.cancel_endpoint_waiter(Some(call_error_info)).0
    }

    pub fn restart_notification_waiter(self) -> Option<TcbRef> {
        let next = list::next_of(self);
        self.with_mut(|t| {
            t.queue = Links::unlinked();
            t.waiting_on = None;
            t.state = ThreadState::Restart;
        });
        next
    }

    pub fn finish_reply_state(self, clear_fault_message: bool, wake: bool) {
        self.with_mut(|t| {
            if clear_fault_message {
                t.clear_fault();
            }
            t.waiting_on = None;
            t.state = if wake {
                ThreadState::Running
            } else {
                ThreadState::Inactive
            };
        });
    }

    pub fn set_blocked_sender(
        self,
        endpoint: EndpointRef,
        is_call: bool,
        badge: u64,
        can_grant: bool,
        can_grant_reply: bool,
        extra_cap_slots: [Option<CteRef>; TCB_SENDER_EXTRA_CAPS],
    ) {
        self.with_mut(|t| {
            t.clear_endpoint_ipc_state();
            t.state = ThreadState::BlockedOnSend;
            t.waiting_on = Some(WaitObject::Endpoint(endpoint));
            t.sender_badge = badge;
            t.sender_can_grant = can_grant;
            t.sender_can_grant_reply = can_grant_reply;
            t.sender_extra_cap_slots = extra_cap_slots;
            t.sender_is_call = is_call;
            t.sender_is_fault = false;
        });
    }

    pub fn set_blocked_receiver(self, endpoint: EndpointRef, can_grant: bool) {
        self.with_mut(|t| {
            t.clear_endpoint_ipc_state();
            t.state = ThreadState::BlockedOnReceive;
            t.waiting_on = Some(WaitObject::Endpoint(endpoint));
            t.receiver_can_grant = can_grant;
        });
    }

    #[allow(clippy::too_many_arguments)]
    pub fn set_blocked_fault_sender(
        self,
        endpoint: EndpointRef,
        badge: u64,
        can_grant: bool,
        can_grant_reply: bool,
        label: u64,
        len: u64,
        mrs: FaultMrs,
    ) {
        self.with_mut(|t| {
            t.clear_endpoint_ipc_state();
            t.state = ThreadState::BlockedOnSend;
            t.waiting_on = Some(WaitObject::Endpoint(endpoint));
            t.sender_badge = badge;
            t.sender_can_grant = can_grant;
            t.sender_can_grant_reply = can_grant_reply;
            t.sender_extra_cap_slots = [None; TCB_SENDER_EXTRA_CAPS];
            t.sender_is_call = true;
            t.sender_is_fault = true;
            t.fault_label = label;
            t.fault_len = len;
            t.fault_mrs = mrs;
        });
    }

    pub fn start_receiver_rendezvous(self) -> bool {
        self.with_mut(|t| {
            t.waiting_on = None;
            t.receiver_can_grant
        })
    }

    pub fn wake_blocked_receiver_after_send(self) {
        self.with_mut(|t| {
            t.waiting_on = None;
            t.clear_endpoint_ipc_state();
            t.state = ThreadState::Running;
        });
    }

    pub fn finish_receiver_rendezvous(self) {
        self.with_mut(|t| {
            t.clear_endpoint_ipc_state();
            t.state = ThreadState::Running;
        });
    }

    pub fn deactivate_queued_call_sender(self) {
        self.with_mut(|t| {
            t.waiting_on = None;
            t.rewind_pc();
            t.state = ThreadState::Inactive;
            t.sender_is_call = false;
        });
    }

    pub fn wake_queued_sender(self) {
        self.with_mut(|t| {
            t.waiting_on = None;
            t.state = ThreadState::Running;
        });
    }

    pub fn finish_call_sender_after_rendezvous(self, blocked_on_reply: bool) {
        self.with_mut(|t| {
            if blocked_on_reply {
                t.state = ThreadState::BlockedOnReply;
            } else {
                t.rewind_pc();
                t.state = ThreadState::Inactive;
            }
            t.waiting_on = None;
            // Keep `sender_is_fault`. Fault Call senders still need it when
            // the matching reply runs `doReplyTransfer`.
        });
    }

    pub fn set_ipc_buffer(self, buffer_uva: UserVa, buffer_cap: Cap) {
        self.with_mut(|t| {
            if buffer_uva.is_zero() {
                t.ipc_buffer_uva = UserVa::ZERO;
                t.ipc_buffer = None;
            } else {
                t.ipc_buffer_uva = buffer_uva;
                // SAFETY: the frame cap names a frame the kernel mapped into
                // its own window when the object was created, which is where
                // `frame_base_ptr` points.
                t.ipc_buffer = unsafe { IpcBuffer::from_kva(buffer_cap.frame_base_ptr()) };
            }
        });
    }

    pub fn set_flags(self, clear: u64, set: u64) -> u64 {
        let flags = self.with_mut(|t| {
            t.flags &= !clear;
            t.flags |= set & TCB_FLAG_MASK;
            t.flags
        });

        if flags & TCB_FLAG_FPU_DISABLED != 0 {
            crate::arch::current::machine::fpu::release(self);
            self.set_fpu_context_enabled(false);
        } else if current() == Some(self) {
            crate::arch::current::machine::fpu::lazy_restore(self);
        }

        flags
    }

    pub fn set_debug_name(self, name: &[u8]) {
        self.with_mut(|t| {
            t.name.fill(0);
            let copy_len = name.len().min(TCB_NAME_LEN - 1);
            t.name[..copy_len].copy_from_slice(&name[..copy_len]);
        });
    }

    // ---- notification binding ----

    pub fn bind_notification(self, notification: NotificationRef) {
        let previous = self.bound_notification();
        if let Some(previous) = previous {
            previous.set_bound_tcb(None);
        }
        self.with_mut(|t| t.bound_notification = Some(notification));
        notification.set_bound_tcb(Some(self));
    }

    pub fn unbind_notification(self) {
        let Some(notification) = self.bound_notification() else {
            return;
        };
        notification.set_bound_tcb(None);
        self.with_mut(|t| t.bound_notification = None);
    }

    /// Drop the binding only if it still names `notification`.
    pub fn clear_bound_notification_if(self, notification: NotificationRef) -> bool {
        self.with_mut(|t| {
            if t.bound_notification != Some(notification) {
                return false;
            }
            t.bound_notification = None;
            true
        })
    }

    // ---- scheduling ----

    /// Add to the tail of this thread's home core's round-robin queue. No-op
    /// if the thread is not runnable or is already queued.
    pub fn enqueue(self) {
        if is_idle_thread(self) || !self.is_runnable() {
            return;
        }
        let core = self.home_core();
        let enqueued = RUNQUEUES[core].with_mut(|queue| {
            if list::contains(queue, self) || !list::is_unlinked(self) {
                return false;
            }
            list::push_back(queue, self);
            true
        });
        if enqueued {
            crate::kernel::smp::wake_core(core);
        }
    }

    /// Unlink from whichever core's round-robin queue holds this thread.
    pub fn dequeue(self) {
        for core in 0..MAX_NUM_NODES {
            let removed = RUNQUEUES[core].with_mut(|queue| list::remove_if_present(queue, self));
            if removed {
                return;
            }
        }
    }

    /// Move to the tail of this thread's home core's queue.
    pub fn rotate_to_tail(self) {
        if is_idle_thread(self) {
            return;
        }
        let core = self.home_core();
        let runnable = self.is_runnable();
        RUNQUEUES[core].with_mut(|queue| {
            if queue.head() == Some(self) && queue.tail() == Some(self) {
                return; // singleton, nothing to do
            }
            if list::remove_if_present(queue, self) && runnable {
                list::push_back(queue, self);
            }
        });
    }

    #[inline]
    pub fn is_runnable_on_current_core(self) -> bool {
        self.with(|t| {
            t.state.is_runnable()
                && core_for_affinity(t.affinity) == crate::kernel::smp::current_core_id()
        })
    }

    /// If the local core trapped while running a TCB whose affinity was moved
    /// to another core, publish it on that core's runqueue before this core
    /// schedules something else.
    pub fn enqueue_if_migrated_from_current_core(self) {
        if is_idle_thread(self) {
            return;
        }
        let migrated = self.with(|t| {
            t.state.is_runnable()
                && core_for_affinity(t.affinity) != crate::kernel::smp::current_core_id()
        });
        if migrated {
            self.enqueue();
        }
    }

    pub fn set_affinity(self, affinity: u8) {
        if is_idle_thread(self) {
            return;
        }
        let (state, old_affinity) = self.with(|t| (t.state, t.affinity));
        if core_for_affinity(old_affinity) != core_for_affinity(affinity) {
            crate::arch::current::machine::fpu::release(self);
        }
        let was_runnable = state.is_runnable();
        let running_core = crate::kernel::smp::current_core_of_tcb(self);
        let migrate_current = running_core.is_some_and(|core| core != affinity as usize);
        if was_runnable {
            self.dequeue();
        }
        self.with_mut(|t| t.affinity = affinity);
        if was_runnable && !migrate_current {
            self.enqueue();
        }
        if migrate_current {
            crate::kernel::smp::wake_current_core_of_tcb(self);
        }
    }

    // ---- lifecycle ----

    /// Detach from any Endpoint or Notification wait list this thread is
    /// queued on because of a prior blocking Send / Recv / Call / Wait.
    ///
    /// `BlockedOnReply` has no wait object; `suspend` deletes the derived
    /// caller cap through `reply::cancel_blocked_on_reply`.
    fn unlink_from_wait_object(self) {
        let Some(wait_object) = self.waiting_on() else {
            return;
        };
        match wait_object {
            WaitObject::Notification(ntfn) => ntfn.remove_waiter(self),
            WaitObject::Endpoint(ep) => ep.remove_waiter(self),
        }
        self.with_mut(|t| {
            t.waiting_on = None;
            t.clear_endpoint_ipc_state();
        });
    }

    /// Mark the TCB non-runnable and take it off the ready queue. The actual
    /// CPU swap happens at the next `kernel_exit()` boundary.
    pub fn suspend(self) {
        if is_idle_thread(self) {
            return;
        }
        crate::kernel::smp::remote_tcb_stall(self);
        // A suspended TCB must leave any EP wait list it is queued on,
        // otherwise the EP would later try to `pop_head` a TCB whose backing
        // slab might have been reused.
        self.unlink_from_wait_object();
        self.dequeue();
        crate::object::reply::cancel_blocked_on_reply(self);
        self.set_state(ThreadState::Inactive);
        crate::kernel::smp::wake_current_core_of_tcb(self);
    }

    pub fn resume(self) {
        if is_idle_thread(self) || !self.state().is_stopped() {
            return;
        }
        self.unlink_from_wait_object();
        crate::object::reply::cancel_blocked_on_reply(self);
        crate::object::reply::setup_reply_master(self);
        self.set_state(ThreadState::Restart);
        self.enqueue();
    }

    /// Wipe a TCB on destruction. Called from `finalize_cap(Thread)`.
    ///
    /// Drops the bound notification, IPC wait state, and runnable state so a
    /// stale handle to this memory cannot remain live during the in-flight
    /// Revoke.
    pub fn finalize(self) {
        crate::kernel::smp::debug_assert_kernel_lock_held();
        // Match seL4 finaliseCap(Thread): unbind the notification first, then
        // run the normal suspend path before clearing Rust-local state.
        self.unbind_notification();
        crate::arch::current::machine::fpu::release(self);
        self.suspend();
        self.with_mut(|t| {
            t.queue = Links::unlinked();
            t.waiting_on = None;
            t.clear_endpoint_ipc_state();
            t.flags = 0;
            t.bound_notification = None;
            t.ipc_buffer_uva = UserVa::ZERO;
            t.ipc_buffer = None;
            t.state = ThreadState::Inactive;
        });
    }
}

/// Reborrow a Thread cap as a TCB handle.
#[inline]
pub fn from_cap(cap: Cap) -> Option<TcbRef> {
    cap.as_thread()
}

/// Initialise a freshly-retyped 2 KiB TCB slab.
///
/// Writes a whole `Tcb` rather than patching the slab's zeroes: that way the
/// resting value of every field is whatever the field's own type says it is,
/// instead of depending on each type encoding "empty" as all-zero bits. Then
/// stamp the bits where zero isn't the right resting value, so a future user
/// restore returns to user mode with interrupts enabled.
///
/// # Safety
/// `tcb_kva` must be the base of a 2 KiB-aligned slab that the caller has
/// just retyped into a TCB object, with nothing else referring to it.
pub unsafe fn init(tcb_kva: u64) {
    // SAFETY: the caller vouches for an aligned, exclusively-owned slab that
    // is large enough, and a plain write initialises it without reading the
    // previous contents.
    unsafe { core::ptr::write(tcb_kva as *mut Tcb, Tcb::zero()) };
    // SAFETY: the write above made this address a live, initialised `Tcb`.
    let tcb: TcbRef = unsafe { ObjRef::from_kva_unchecked(tcb_kva) };
    tcb.with_mut(|t| {
        t.affinity = crate::kernel::smp::current_core_id() as u8;
        t.time_slice = DEFAULT_TIME_SLICE;
        sel4_arch::init_user_context(&mut t.context);
    });
    crate::object::reply::setup_reply_master(tcb);
}

/// Pick the next round-robin ready TCB, or `None` if the queue is empty.
pub fn schedule() -> Option<TcbRef> {
    let candidate = peek_schedule()?;
    candidate.dequeue();
    Some(candidate)
}

/// Return the next round-robin ready TCB without consuming it. Stale
/// non-runnable queue entries are still discarded.
pub fn peek_schedule() -> Option<TcbRef> {
    loop {
        let candidate = schedule_head()?;
        if candidate.is_runnable_on_current_core() {
            return Some(candidate);
        }
        candidate.dequeue();
    }
}

fn schedule_head() -> Option<TcbRef> {
    let core = crate::kernel::smp::current_core_id();
    RUNQUEUES[core].with_ref(|queue| queue.head())
}
