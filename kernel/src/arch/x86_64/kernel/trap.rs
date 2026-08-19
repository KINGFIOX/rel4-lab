use crate::arch::x86_64::sel4_arch::UserContext;
use crate::object::cap::Cap;

pub unsafe fn restore_user_context(_ctx: *mut UserContext) -> ! {
    crate::arch::x86_64::kernel::boot::halt()
}

pub unsafe fn restore_user_context_with_kernel_lock(
    ctx: *mut UserContext,
    kernel_lock: crate::kernel::smp::KernelLockGuard,
) -> ! {
    kernel_lock.defer_unlock_for_user_restore();
    unsafe { restore_user_context(ctx) }
}

pub fn install_trap_vector() {}

pub fn init_timer() {}

pub fn service_due_timer_interrupts() -> bool {
    false
}

pub fn idle_scheduler_loop() -> ! {
    crate::arch::x86_64::kernel::boot::halt()
}

pub fn send_cap_fault_ipc(_uc: &mut UserContext, _addr: u64, _in_recv_phase: bool) -> bool {
    false
}

pub fn same_object_as(_left: Cap, _right: Cap) -> bool {
    false
}
