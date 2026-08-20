//! Per-core trap scratch addressed through `sscratch`.
//!
//! `trap.S` relies on this exact layout. Keep the field order in sync with
//! the `TRAP_SCRATCH_*` offsets in that file.

use core::cell::UnsafeCell;
use core::mem::size_of;

#[repr(C)]
pub struct TrapScratch {
    pub kernel_stack_top: usize,
    pub user_context: usize,
    pub saved_user_sp: usize,
    pub saved_user_t1: usize,
    pub saved_user_t2: usize,
    pub core_id: usize,
    pub cpu_id: usize,
}

const _: () = {
    assert!(size_of::<TrapScratch>() == 7 * size_of::<usize>());
    assert!(core::mem::offset_of!(TrapScratch, kernel_stack_top) == 0);
    assert!(core::mem::offset_of!(TrapScratch, user_context) == 1 * size_of::<usize>());
    assert!(core::mem::offset_of!(TrapScratch, saved_user_sp) == 2 * size_of::<usize>());
    assert!(core::mem::offset_of!(TrapScratch, saved_user_t1) == 3 * size_of::<usize>());
    assert!(core::mem::offset_of!(TrapScratch, saved_user_t2) == 4 * size_of::<usize>());
    assert!(core::mem::offset_of!(TrapScratch, core_id) == 5 * size_of::<usize>());
    assert!(core::mem::offset_of!(TrapScratch, cpu_id) == 6 * size_of::<usize>());
};

impl TrapScratch {
    pub const fn new() -> Self {
        Self {
            kernel_stack_top: 0,
            user_context: 0,
            saved_user_sp: 0,
            saved_user_t1: 0,
            saved_user_t2: 0,
            core_id: usize::MAX,
            cpu_id: usize::MAX,
        }
    }
}

pub struct TrapScratchCell(UnsafeCell<TrapScratch>);

// SAFETY: a core's scratch area is written by that core's boot path and then
// only by its own trap entry/exit assembly; other cores never touch it.
unsafe impl Sync for TrapScratchCell {}

impl TrapScratchCell {
    pub const fn new() -> Self {
        Self(UnsafeCell::new(TrapScratch::new()))
    }

    pub fn get(&self) -> *mut TrapScratch {
        self.0.get()
    }
}

/// The running core's scratch area, or `None` before its boot path installed
/// one.
///
/// Reading is safe because the fields the kernel reads through this
/// (`core_id`, `cpu_id`, `kernel_stack_top`) are written once during that
/// core's boot and never change afterwards. The fields the assembly writes on
/// every trap are not exposed this way.
pub fn current_trap_scratch() -> Option<&'static TrapScratch> {
    let scratch = crate::arch::riscv64::machine::current_scratch() as *const TrapScratch;
    if scratch.is_null() {
        return None;
    }
    // SAFETY: the boot path installed the address of this core's
    // `TrapScratchCell`, which lives in a `static` for the whole run.
    Some(unsafe { &*scratch })
}

pub unsafe fn init_trap_scratch(
    scratch: *mut TrapScratch,
    kernel_stack_top: usize,
    core_id: usize,
    cpu_id: usize,
) {
    // SAFETY: forwarded to the caller.
    unsafe {
        (*scratch).kernel_stack_top = kernel_stack_top;
        (*scratch).user_context = 0;
        (*scratch).saved_user_sp = 0;
        (*scratch).saved_user_t1 = 0;
        (*scratch).saved_user_t2 = 0;
        (*scratch).core_id = core_id;
        (*scratch).cpu_id = cpu_id;
        crate::arch::riscv64::machine::set_current_scratch(scratch as usize);
    }
}
