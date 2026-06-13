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
/// We establish a temporary stack, clear `.bss` on the boot CPU, then call the
/// Rust `init_kernel` entry with those registers still containing the handoff
/// tuple.
#[unsafe(naked)]
#[unsafe(no_mangle)]
#[unsafe(link_section = ".boot.text")]
pub unsafe extern "C" fn _start() -> ! {
    naked_asm!(
        "ibar 0",
        "dbar 0",

        "la.local $t0, __stack_top",
        "move     $sp, $t0",

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

        init_kernel = sym init_kernel,
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

pub fn halt() -> ! {
    loop {
        unsafe {
            core::arch::asm!("idle 0", options(nomem, nostack));
        }
    }
}
