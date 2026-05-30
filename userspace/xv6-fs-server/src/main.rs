#![no_std]
#![no_main]
#![deny(unsafe_attr_outside_unsafe)]
#![deny(unsafe_op_in_unsafe_fn)]

use core::panic::PanicInfo;

use sel4_user::{
    IpcMessage, halt_loop, init_ipc_buffer, log, msg_info, msg_label, print_u64, sel4_call,
    sel4_recv, sel4_reply_recv,
};
use xv6_abi::{
    DISK_OP_GET_INFO, FS_BLOCK_SIZE, FS_OP_INIT, FS_OP_OPEN, VIRTIO_BLK_SECTOR_SIZE,
    XV6_ABI_VERSION, XV6_DISK_ENDPOINT_CPTR, XV6_EINVAL, XV6_ENOSYS, XV6_FS_MAGIC,
    XV6_FS_TO_DISK_PROTOCOL, XV6_HOST_TO_FS_PROTOCOL, XV6_OK, XV6_SERVICE_ENDPOINT_CPTR,
};

#[unsafe(no_mangle)]
pub extern "C" fn _start(ipc_buffer: usize) -> ! {
    init_ipc_buffer(ipc_buffer as u64);
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
    let mut msg = unsafe { sel4_recv(XV6_SERVICE_ENDPOINT_CPTR) };
    loop {
        let reply_mrs = handle_request(&msg);
        msg =
            unsafe { sel4_reply_recv(XV6_SERVICE_ENDPOINT_CPTR, msg_info(0, 0, 0, 4), &reply_mrs) };
    }
}

fn handle_request(msg: &IpcMessage) -> [u64; 4] {
    match msg_label(msg.info) {
        FS_OP_INIT => handle_init(msg),
        op => {
            log("xv6-fs-server: unsupported op=");
            print_u64(op);
            log("\n");
            [XV6_ENOSYS, 0, 0, 0]
        }
    }
}

fn handle_init(msg: &IpcMessage) -> [u64; 4] {
    if msg.mrs[0] != XV6_HOST_TO_FS_PROTOCOL || msg.mrs[1] != XV6_ABI_VERSION {
        log("xv6-fs-server: bad init protocol\n");
        return [XV6_EINVAL, 0, 0, 0];
    }
    log("xv6-fs-server: init from host\n");
    let disk = unsafe {
        sel4_call(
            XV6_DISK_ENDPOINT_CPTR,
            msg_info(DISK_OP_GET_INFO, 0, 0, 2),
            &[XV6_FS_TO_DISK_PROTOCOL, XV6_ABI_VERSION],
        )
    };
    if msg_label(disk.info) != 0 || disk.mrs[0] != XV6_OK {
        log("xv6-fs-server: disk info failed status=");
        print_u64(disk.mrs[0]);
        log("\n");
        return [disk.mrs[0], 0, 0, 0];
    }
    if disk.mrs[1] != VIRTIO_BLK_SECTOR_SIZE as u64 {
        log("xv6-fs-server: unexpected disk sector size\n");
        return [XV6_EINVAL, 0, 0, 0];
    }
    if disk.mrs[3] != XV6_FS_MAGIC as u64 {
        log("xv6-fs-server: unexpected superblock magic=");
        print_u64(disk.mrs[3]);
        log("\n");
        return [XV6_EINVAL, 0, 0, 0];
    }
    log("xv6-fs-server: disk ready sector=");
    print_u64(disk.mrs[1]);
    log(" fs-block=");
    print_u64(FS_BLOCK_SIZE as u64);
    log(" blocks=");
    print_u64(disk.mrs[2]);
    log(" magic=");
    print_u64(disk.mrs[3]);
    log("\n");
    [XV6_OK, disk.mrs[1], FS_BLOCK_SIZE as u64, disk.mrs[2]]
}

#[panic_handler]
fn panic(_: &PanicInfo) -> ! {
    log("xv6-fs-server: panic\n");
    halt_loop()
}
