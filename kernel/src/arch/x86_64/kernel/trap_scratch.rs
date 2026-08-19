//! Per-core trap scratch for the staged x86_64 backend.

use core::cell::UnsafeCell;

#[repr(C)]
pub struct TrapScratch {
    pub kernel_stack_top: usize,
    pub user_context: usize,
    pub core_id: usize,
    pub cpu_id: usize,
}

impl TrapScratch {
    pub const fn new() -> Self {
        Self {
            kernel_stack_top: 0,
            user_context: 0,
            core_id: usize::MAX,
            cpu_id: usize::MAX,
        }
    }
}

pub struct TrapScratchCell(UnsafeCell<TrapScratch>);

unsafe impl Sync for TrapScratchCell {}

impl TrapScratchCell {
    pub const fn new() -> Self {
        Self(UnsafeCell::new(TrapScratch::new()))
    }

    pub fn get(&self) -> *mut TrapScratch {
        self.0.get()
    }
}

pub unsafe fn init_trap_scratch(
    scratch: *mut TrapScratch,
    kernel_stack_top: usize,
    core_id: usize,
    cpu_id: usize,
) {
    unsafe {
        (*scratch).kernel_stack_top = kernel_stack_top;
        (*scratch).user_context = 0;
        (*scratch).core_id = core_id;
        (*scratch).cpu_id = cpu_id;
        crate::arch::x86_64::machine::set_current_scratch(scratch as usize);
    }
}
