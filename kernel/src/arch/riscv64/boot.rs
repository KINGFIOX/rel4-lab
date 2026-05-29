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
/// information. Set up the kernel stack, clear bss, then hand off to
/// `init_kernel` (regular C ABI fn — a0..a7 are preserved across the
/// `jal`).
///
/// Secondary harts are parked before global kernel init. The current Rust
/// kernel still has single global boot state, so only core 0 may clear BSS
/// and bring up the rootserver.
#[unsafe(naked)]
#[unsafe(no_mangle)]
#[unsafe(link_section = ".boot.text")]
pub unsafe extern "C" fn _start() -> ! {
    naked_asm!(
        "fence.i",

        // elfloader passes core_id in a7. Until the SMP scheduler path is
        // brought up, park secondary harts before they touch shared state.
        "bnez   a7, 4f",

        // Set sp to top of kernel stack (set up by the linker script in .bss).
        "la     sp, __stack_top",

        // Zero sscratch (used for trap entry user-vs-kernel discrimination).
        "csrw   sscratch, x0",

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
        "la     sp, __stack_top",

        // Call init_kernel(a0..a7). The elfloader's args are already in place.
        "call   {init_kernel}",

        // init_kernel currently returns; for M2 we just spin afterwards.
        "3:",
        "wfi",
        "j      3b",

        "4:",
        "wfi",
        "j      4b",

        init_kernel = sym init_kernel,
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

    crate::println!("microkernel: Rust kernel booted (S-mode, Sv39)");
    crate::println!(
        "  hart_id={} core_id={} dtb=0x{:x} ({} bytes)",
        hart_id,
        core_id,
        dtb_pa,
        dtb_size
    );
    crate::println!(
        "  user image: pa=[0x{:x}..0x{:x}], pv_offset=0x{:x}, entry=0x{:x}",
        user_pstart,
        user_pend,
        pv_offset,
        user_ventry
    );

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

/// Halt the calling hart: enter low-power wait-for-interrupt loop forever.
pub fn halt() -> ! {
    loop {
        unsafe {
            core::arch::asm!("wfi");
        }
    }
}
