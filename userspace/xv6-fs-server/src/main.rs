#![no_std]
#![no_main]
#![deny(unsafe_attr_outside_unsafe)]
#![deny(unsafe_op_in_unsafe_fn)]

use core::panic::PanicInfo;
use core::sync::atomic::{Ordering, fence};

mod block;
mod dir;
mod inode;
mod ops;
mod types;

use block::{
    exercise_disk_write, handle_transactional, read_disk_block, read_superblock_from_shared,
    recover_log,
};
use dir::{count_dir_entries, lookup_root_name};
use inode::{free_inode, read_inode};
use ops::{
    handle_chdir, handle_close, handle_exec_lookup, handle_fstat, handle_link, handle_mkdir,
    handle_mknod, handle_open, handle_read, handle_readdir, handle_retain, handle_unlink,
    handle_write,
};
use sel4_user::{
    IpcMessage, halt_loop, init_ipc_buffer, log, msg_info, msg_label, print_u64, sel4_call,
    sel4_recv, sel4_reply_recv,
};
use types::{FS_STATE, FsState, XV6_LOG_MAX_BLOCKS};
use xv6_abi::{
    DISK_OP_GET_INFO, FS_BLOCK_SIZE, FS_OP_CHDIR, FS_OP_CLOSE, FS_OP_EXEC_LOOKUP, FS_OP_FSTAT,
    FS_OP_INIT, FS_OP_LINK, FS_OP_MKDIR, FS_OP_MKNOD, FS_OP_OPEN, FS_OP_READ, FS_OP_READDIR,
    FS_OP_RETAIN, FS_OP_UNLINK, FS_OP_WRITE, T_DIR, VIRTIO_BLK_SECTOR_SIZE, XV6_ABI_VERSION,
    XV6_DISK_COMPLETION_BADGE, XV6_DISK_ENDPOINT_CPTR, XV6_EINVAL, XV6_ENOSYS, XV6_FS_MAGIC,
    XV6_FS_ROOT_INUM, XV6_FS_TO_DISK_PROTOCOL, XV6_HOST_TO_FS_PROTOCOL, XV6_OK,
    XV6_SERVICE_ENDPOINT_CPTR,
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
    let mut reply_pending = false;
    let mut reply_mrs = [0u64; 4];
    loop {
        let msg = if reply_pending {
            reply_pending = false;
            unsafe { sel4_reply_recv(XV6_SERVICE_ENDPOINT_CPTR, msg_info(0, 0, 0, 4), &reply_mrs) }
        } else {
            unsafe { sel4_recv(XV6_SERVICE_ENDPOINT_CPTR) }
        };
        if (msg.badge & XV6_DISK_COMPLETION_BADGE) != 0 {
            continue;
        }

        reply_mrs = handle_request(&msg);
        reply_pending = true;
    }
}

fn handle_request(msg: &IpcMessage) -> [u64; 4] {
    match msg_label(msg.info) {
        FS_OP_INIT => handle_init(msg),
        FS_OP_OPEN => handle_transactional(|| handle_open(msg)),
        FS_OP_CLOSE => handle_transactional(|| handle_close(msg)),
        FS_OP_READ => handle_read(msg),
        FS_OP_WRITE => handle_transactional(|| handle_write(msg)),
        FS_OP_FSTAT => handle_fstat(msg),
        FS_OP_CHDIR => handle_chdir(msg),
        FS_OP_EXEC_LOOKUP => handle_exec_lookup(msg),
        FS_OP_READDIR => handle_readdir(msg),
        FS_OP_RETAIN => handle_retain(msg),
        FS_OP_UNLINK => handle_transactional(|| handle_unlink(msg)),
        FS_OP_LINK => handle_transactional(|| handle_link(msg)),
        FS_OP_MKDIR => handle_transactional(|| handle_mkdir(msg)),
        FS_OP_MKNOD => handle_transactional(|| handle_mknod(msg)),
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

    if !read_disk_block(1) {
        log("xv6-fs-server: superblock read failed\n");
        return [XV6_EINVAL, 0, 0, 0];
    }
    fence(Ordering::SeqCst);
    let superblock = read_superblock_from_shared();
    log("xv6-fs-server: superblock magic=");
    print_u64(superblock.magic as u64);
    log("\n");
    if superblock.magic != XV6_FS_MAGIC {
        log("xv6-fs-server: unexpected superblock magic=");
        print_u64(superblock.magic as u64);
        log("\n");
        return [XV6_EINVAL, 0, 0, 0];
    }
    if superblock.size != disk.mrs[2] as u32 {
        log("xv6-fs-server: superblock/disk block mismatch\n");
        return [XV6_EINVAL, 0, 0, 0];
    }
    if superblock.nlog == 0
        || superblock.nlog as usize > XV6_LOG_MAX_BLOCKS + 1
        || superblock.logstart.saturating_add(superblock.nlog) > superblock.size
    {
        log("xv6-fs-server: unsupported log geometry nlog=");
        print_u64(superblock.nlog as u64);
        log("\n");
        return [XV6_EINVAL, 0, 0, 0];
    }

    unsafe {
        FS_STATE = FsState {
            ready: true,
            superblock,
        };
    }
    if !recover_log() {
        log("xv6-fs-server: log recovery failed\n");
        return [XV6_EINVAL, 0, 0, 0];
    }
    if !reclaim_orphan_inodes() {
        log("xv6-fs-server: orphan reclaim failed\n");
        return [XV6_EINVAL, 0, 0, 0];
    }

    if superblock.size == 0 || !exercise_disk_write(superblock.size - 1) {
        log("xv6-fs-server: disk write verification failed\n");
        return [XV6_EINVAL, 0, 0, 0];
    }

    let Some(root) = read_inode(XV6_FS_ROOT_INUM) else {
        log("xv6-fs-server: root inode read failed\n");
        return [XV6_EINVAL, 0, 0, 0];
    };
    if root.typ != T_DIR as i16 {
        log("xv6-fs-server: root inode is not a directory\n");
        return [XV6_EINVAL, 0, 0, 0];
    }
    let root_entries = count_dir_entries(&root);
    let Some((readme_ino, readme)) = lookup_root_name(b"README") else {
        log("xv6-fs-server: README not found in fs.img root\n");
        return [XV6_EINVAL, 0, 0, 0];
    };
    log("xv6-fs-server: root entries=");
    print_u64(root_entries as u64);
    log(" README ino=");
    print_u64(readme_ino as u64);
    log(" size=");
    print_u64(readme.size as u64);
    log(" nlink=");
    print_u64(readme.nlink as u64);
    log("\n");

    log("xv6-fs-server: disk ready sector=");
    print_u64(disk.mrs[1]);
    log(" fs-block=");
    print_u64(FS_BLOCK_SIZE as u64);
    log(" blocks=");
    print_u64(disk.mrs[2]);
    log(" magic=");
    print_u64(superblock.magic as u64);
    log("\n");
    [XV6_OK, disk.mrs[1], FS_BLOCK_SIZE as u64, disk.mrs[2]]
}

fn reclaim_orphan_inodes() -> bool {
    let state = unsafe { FS_STATE };
    if !state.ready {
        return false;
    }
    let mut inum = 1u32;
    while inum < state.superblock.ninodes {
        let Some(inode) = read_inode(inum) else {
            return false;
        };
        if inode.typ != 0 && inode.nlink == 0 {
            log("ireclaim: orphaned inode ");
            print_u64(inum as u64);
            log("\n");
            if handle_transactional(|| {
                if free_inode(inum) {
                    [XV6_OK, 0, 0, 0]
                } else {
                    [XV6_EINVAL, 0, 0, 0]
                }
            })[0]
                != XV6_OK
            {
                return false;
            }
        }
        inum += 1;
    }
    true
}

#[panic_handler]
fn panic(_: &PanicInfo) -> ! {
    log("xv6-fs-server: panic\n");
    halt_loop()
}
