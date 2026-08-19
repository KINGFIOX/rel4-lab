#![no_std]
#![no_main]
#![deny(unsafe_attr_outside_unsafe)]
#![deny(unsafe_op_in_unsafe_fn)]

use core::panic::PanicInfo;
use core::ptr;

use sel4_user::{
    BootInfo, LABEL_UNTYPED_RETYPE, OBJ_ENDPOINT, ROOT_CNODE, error, halt_loop, info,
    init_ipc_buffer, init_logger, sel4_nb_recv,
};

#[unsafe(no_mangle)]
pub extern "C" fn _start(bootinfo: usize) -> ! {
    unsafe {
        clear_bss();
    }
    let bi = unsafe { &*(bootinfo as *const BootInfo) };
    init_ipc_buffer(bi.ipc_buffer);
    init_logger();
    info!("hello-rootserver: boot");

    let untyped = first_ram_untyped(bi);
    let dest_slot = bi.empty.start;
    if untyped == 0 || dest_slot == 0 || dest_slot >= bi.empty.end {
        error!("hello-rootserver: missing untyped or empty CNode slot");
        halt_loop();
    }

    sel4_user::call_checked(
        untyped,
        LABEL_UNTYPED_RETYPE,
        &[ROOT_CNODE],
        &[OBJ_ENDPOINT, 0, 0, 0, dest_slot, 1],
    );
    let _ = unsafe { sel4_nb_recv(dest_slot) };
    info!("hello-rootserver: ok");
    halt_loop();
}

fn first_ram_untyped(bi: &BootInfo) -> u64 {
    let mut slot = bi.untyped.start;
    let mut index = 0usize;
    while slot < bi.untyped.end && index < bi.untyped_list.len() {
        if bi.untyped_list[index].is_device == 0 {
            return slot;
        }
        slot += 1;
        index += 1;
    }
    0
}

unsafe fn clear_bss() {
    unsafe extern "C" {
        static __bss_start: u8;
        static __bss_end: u8;
    }
    unsafe {
        let start = core::ptr::addr_of!(__bss_start) as usize;
        let end = core::ptr::addr_of!(__bss_end) as usize;
        ptr::write_bytes(start as *mut u8, 0, end.saturating_sub(start));
    }
}

#[panic_handler]
fn panic(info: &PanicInfo<'_>) -> ! {
    sel4_user::error!("hello-rootserver panic: {info}");
    halt_loop();
}
