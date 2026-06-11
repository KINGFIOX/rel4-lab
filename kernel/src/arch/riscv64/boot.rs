//! Boot entry, runs in S-mode after the seL4 elfloader handed control over
//! with Sv39 paging already enabled. The elfloader's call signature is:
//!
//! ```text
//! init_kernel(user_pstart, user_pend, pv_offset,
//!             user_ventry, dtb_pa, dtb_size, hart_id, core_id);
//! ```
//!
//! (see `kernel/src/arch/riscv/head.S` in the seL4 source tree.)

use core::arch::naked_asm;

unsafe extern "C" {
    static __bss_start: u8;
    static __bss_end: u8;
    static __stack_top: u8;
}

/// Kernel entry. Reached from elfloader with a0..a7 carrying user image
/// information. Set up a per-core kernel stack, clear bss on core 0, then
/// hand off to `init_kernel` (regular C ABI fn — a0..a7 are preserved across
/// the `jal`).
///
/// Secondary harts wait until core 0 publishes the ready magic in
/// `SECONDARY_BOOT_READY`, then enter Rust with their own stacks and join the
/// per-core scheduler.
#[unsafe(naked)]
#[unsafe(no_mangle)]
#[unsafe(link_section = ".boot.text")]
pub unsafe extern "C" fn _start() -> ! {
    naked_asm!(
        "fence.i",

        // Clear sstatus.FS before any hart enters Rust or parks. This is
        // CSR hygiene only: the kernel is built for IMAC and does not save,
        // restore, or execute floating-point state.
        "li     t0, 0x6000",
        "csrc   sstatus, t0",

        // elfloader passes core_id in a7. Each hart gets a 64 KiB stack from
        // the linker-reserved stack region, counted down from __stack_top.
        "la     t0, __stack_top",
        "li     t1, 65536",
        "mul    t1, a7, t1",
        "sub    sp, t0, t1",

        // Zero sscratch (used for trap entry user-vs-kernel discrimination).
        "csrw   sscratch, x0",

        // Only core 0 may clear .bss and bring up shared kernel state.
        "bnez   a7, 4f",

        // Clear .bss. The stack lives inside .bss so we must zero it before
        // using sp — but we already set sp, so do the clear after with t0/t1.
        // Note: the stack itself lives in this range; zeroing it is harmless
        // because we haven't pushed anything yet.
        "la     t0, __bss_start",
        "la     t1, __bss_end",
        "1:",
        "bgeu   t0, t1, 2f",
        "sd     zero, 0(t0)",
        "addi   t0, t0, 8",
        "j      1b",
        "2:",

        // Re-establish sp after wiping bss (in case clearing trampled it).
        "la     t0, __stack_top",
        "mv     sp, t0",

        // Call init_kernel(a0..a7). The elfloader's args are already in place.
        "call   {init_kernel}",

        // init_kernel currently returns; for M2 we just spin afterwards.
        "3:",
        "wfi",
        "j      3b",

        // Secondary hart path: wait until core 0 has finished global init.
        "4:",
        "la     t0, {secondary_boot_ready}",
        "li     t2, {secondary_boot_ready_magic}",
        "5:",
        "ld     t1, 0(t0)",
        "bne    t1, t2, 5b",
        "fence  r, rw",
        "call   {init_secondary_hart}",
        "6:",
        "wfi",
        "j      6b",

        init_kernel = sym init_kernel,
        init_secondary_hart = sym init_secondary_hart,
        secondary_boot_ready = sym crate::kernel::smp::SECONDARY_BOOT_READY,
        secondary_boot_ready_magic = const crate::kernel::smp::SECONDARY_BOOT_READY_MAGIC,
    );
}

/// First Rust function after we have a stack and zeroed bss.
///
/// At this point Sv39 paging is enabled, the kernel ELF is mapped at its
/// link-time VA, and we are executing in S-mode. The argument list is the
/// 8-tuple defined by the seL4 elfloader (see `head.S`).
#[unsafe(no_mangle)]
pub extern "C" fn init_kernel(
    user_pstart: usize,
    user_pend: usize,
    pv_offset: usize,
    user_ventry: usize,
    dtb_pa: usize,
    dtb_size: usize,
    hart_id: usize,
    core_id: usize,
) -> ! {
    crate::arch::riscv64::trap::disable_fpu_access();

    // Touch the linker symbols so they don't get stripped.
    let _ = unsafe {
        (
            &__bss_start as *const u8,
            &__bss_end as *const u8,
            &__stack_top as *const u8,
        )
    };

    let args = crate::kernel::boot::BootArgs {
        user_pstart,
        user_pend,
        pv_offset,
        user_ventry,
        dtb_pa,
        dtb_size,
        hart_id,
        core_id,
    };
    crate::kernel::boot::bringup_rootserver(&args);
}

/// Secondary hart entry after core 0 has completed global boot state.
#[unsafe(no_mangle)]
pub extern "C" fn init_secondary_hart(
    _user_pstart: usize,
    _user_pend: usize,
    _pv_offset: usize,
    _user_ventry: usize,
    _dtb_pa: usize,
    _dtb_size: usize,
    hart_id: usize,
    core_id: usize,
) -> ! {
    crate::arch::riscv64::trap::disable_fpu_access();
    crate::kernel::smp::init_current_hart(hart_id, core_id);
    if let Some(satp) = crate::kernel::smp::kernel_satp() {
        unsafe { crate::arch::riscv64::vspace::switch_satp(satp) };
    }
    crate::arch::riscv64::trap::install_trap_vector();
    crate::arch::riscv64::trap::init_timer();
    crate::arch::riscv64::trap::idle_scheduler_loop()
}

/// Halt the calling hart: enter low-power wait-for-interrupt loop forever.
pub fn halt() -> ! {
    loop {
        unsafe {
            core::arch::asm!("wfi");
        }
    }
}
