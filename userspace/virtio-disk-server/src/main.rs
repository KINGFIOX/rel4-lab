#![no_std]
#![no_main]
#![deny(unsafe_attr_outside_unsafe)]
#![deny(unsafe_op_in_unsafe_fn)]

use core::panic::PanicInfo;

use sel4_user::{BootInfo, halt_loop, init_ipc_buffer, log, print_u64};
use xv6_abi::{DISK_OP_GET_INFO, VIRTIO_BLK_SECTOR_SIZE, XV6_ABI_VERSION, XV6_FS_TO_DISK_PROTOCOL};

#[unsafe(no_mangle)]
pub extern "C" fn _start(bootinfo: usize) -> ! {
    let bi = unsafe { &*(bootinfo as *const BootInfo) };
    init_ipc_buffer(bi.ipc_buffer);
    log("virtio-disk-server: boot\n");
    log("virtio-disk-server: protocol=");
    print_u64(XV6_FS_TO_DISK_PROTOCOL);
    log(" abi=");
    print_u64(XV6_ABI_VERSION);
    log(" sector=");
    print_u64(VIRTIO_BLK_SECTOR_SIZE as u64);
    log(" first-op=");
    print_u64(DISK_OP_GET_INFO);
    log("\n");
    log("virtio-disk-server: waiting for fs-server client hookup\n");
    halt_loop()
}

#[panic_handler]
fn panic(_: &PanicInfo) -> ! {
    log("virtio-disk-server: panic\n");
    halt_loop()
}
