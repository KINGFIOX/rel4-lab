use core::arch::asm;

unsafe extern "C" {
    static __bss_start: u8;
    static __bss_end: u8;
    static __stack_top: u8;
}

#[unsafe(no_mangle)]
#[unsafe(link_section = ".boot.text")]
pub extern "C" fn _start() -> ! {
    halt()
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

pub fn halt() -> ! {
    loop {
        unsafe {
            asm!("hlt", options(nomem, nostack));
        }
    }
}
