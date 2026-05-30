#![no_std]
#![no_main]
#![deny(unsafe_attr_outside_unsafe)]
#![deny(unsafe_op_in_unsafe_fn)]

use core::panic::PanicInfo;

use sel4_user::{BootInfo, halt_loop, init_ipc_buffer, log, print_u64};
use xv6_abi::{FS_BLOCK_SIZE, FS_OP_OPEN, XV6_ABI_VERSION, XV6_HOST_TO_FS_PROTOCOL};

#[unsafe(no_mangle)]
pub extern "C" fn _start(bootinfo: usize) -> ! {
    let bi = unsafe { &*(bootinfo as *const BootInfo) };
    init_ipc_buffer(bi.ipc_buffer);
    log("xv6-fs-server: boot\n");
    log("xv6-fs-server: protocol=");
    print_u64(XV6_HOST_TO_FS_PROTOCOL);
    log(" abi=");
    print_u64(XV6_ABI_VERSION);
    log(" block=");
    print_u64(FS_BLOCK_SIZE as u64);
    log(" first-op=");
    print_u64(FS_OP_OPEN);
    log("\n");
    log("xv6-fs-server: waiting for xv6-host and disk-server hookup\n");
    halt_loop()
}

#[panic_handler]
fn panic(_: &PanicInfo) -> ! {
    log("xv6-fs-server: panic\n");
    halt_loop()
}
