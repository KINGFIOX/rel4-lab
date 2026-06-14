//! LoongArch64 boot entry.
//!
//! This is the first executable LoongArch kernel stage. It mirrors the
//! repository's current seL4 elfloader-style eight-argument handoff shape and
//! enters the shared rootserver bring-up path.

use core::arch::naked_asm;

unsafe extern "C" {
    static __bss_start: u8;
    static __bss_end: u8;
    static __stack_top: u8;
}

/// Kernel entry reached from the future LoongArch elfloader handoff.
///
/// The LoongArch psABI passes the first eight integer arguments in `$a0..$a7`.
/// We establish a per-hart stack, clear `.bss` only on the boot CPU, then call
/// the Rust entry points with those registers still containing the handoff
/// tuple.
#[unsafe(naked)]
#[unsafe(no_mangle)]
#[unsafe(link_section = ".boot.text")]
pub unsafe extern "C" fn _start() -> ! {
    naked_asm!(
        "ibar 0",
        "dbar 0",

        // elfloader passes core_id in a7. Each hart gets a 64 KiB stack from
        // the linker-reserved stack region, counted down from __stack_top.
        "la.local $t0, __stack_top",
        "li.d     $t1, 65536",
        "mul.d    $t1, $a7, $t1",
        "sub.d    $sp, $t0, $t1",

        // Clear ks0, the LoongArch trap scratch CSR, until init_current_hart()
        // installs the real per-hart TrapScratch pointer.
        "csrwr    $zero, 0x030",

        // Only core 0 may clear .bss and bring up shared kernel state.
        "bnez     $a7, 4f",

        "la.local $t0, __bss_start",
        "la.local $t1, __bss_end",
        "1:",
        "bgeu     $t0, $t1, 2f",
        "st.d     $zero, $t0, 0",
        "addi.d   $t0, $t0, 8",
        "b        1b",
        "2:",

        "la.local $t0, __stack_top",
        "move     $sp, $t0",
        "bl       {init_kernel}",

        "3:",
        "idle     0",
        "b        3b",

        // Secondary hart path: wait until core 0 has finished global init.
        "4:",
        "la.local $t0, {secondary_boot_ready}",
        "li.d     $t2, {secondary_boot_ready_magic}",
        "5:",
        "ld.d     $t1, $t0, 0",
        "bne      $t1, $t2, 5b",
        "dbar     0",
        "bl       {init_secondary_hart}",
        "6:",
        "idle     0",
        "b        6b",

        init_kernel = sym init_kernel,
        init_secondary_hart = sym init_secondary_hart,
        secondary_boot_ready = sym crate::kernel::smp::SECONDARY_BOOT_READY,
        secondary_boot_ready_magic = const crate::kernel::smp::SECONDARY_BOOT_READY_MAGIC,
    );
}

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
    crate::kernel::boot::bringup_rootserver(&args)
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
    crate::kernel::smp::init_current_hart(hart_id, core_id);
    crate::arch::loongarch64::fpu::init_current_core();
    if let Some(satp) = crate::kernel::smp::kernel_satp() {
        unsafe { crate::arch::loongarch64::vspace::switch_satp(satp) };
    }
    crate::arch::loongarch64::trap::install_trap_vector();
    crate::arch::loongarch64::trap::init_timer();
    crate::arch::loongarch64::irq::init_current_core();
    crate::arch::loongarch64::trap::idle_scheduler_loop()
}

pub fn halt() -> ! {
    loop {
        unsafe {
            core::arch::asm!("idle 0", options(nomem, nostack));
        }
    }
}
