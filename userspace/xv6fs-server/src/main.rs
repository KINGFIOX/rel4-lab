#![no_std]
#![no_main]
#![deny(unsafe_attr_outside_unsafe)]
#![deny(unsafe_op_in_unsafe_fn)]

use core::panic::PanicInfo;
use core::sync::atomic::{Ordering, fence};

mod block;
mod continuation;
mod dir;
mod disk;
mod inode;
mod ops;
mod types;

use block::{
    exercise_disk_write, handle_transactional, read_disk_block, read_superblock_from_shared,
    recover_log,
};
use dir::{count_dir_entries, lookup_root_name};
use disk::exercise_concurrent_reads;
use inode::{free_inode, read_inode};
use ops::{handle_link, handle_mkdir, handle_mknod, handle_unlink};
use sel4_user::{
    IpcMessage, error, halt_loop, info, init_ipc_buffer, init_logger, msg_info, msg_label, rt,
    sel4_call, warn,
};
use types::{FS_STATE, FsState, XV6_LOG_MAX_BLOCKS};
use xv6_abi::{
    DiskRequestOp, FS_BLOCK_SIZE, VIRTIO_BLK_SECTOR_SIZE, XV6_ABI_VERSION, XV6_DISK_ENDPOINT_CPTR,
    XV6_FS_MAGIC, XV6_FS_ROOT_INUM, XV6_SERVER_RECV_REPLY_CPTR, XV6_SERVICE_ENDPOINT_CPTR,
    Xv6Badge, Xv6FileType, Xv6FsOp, Xv6Protocol, Xv6Status,
};

#[unsafe(no_mangle)]
pub extern "C" fn _start(ipc_buffer: usize) -> ! {
    init_ipc_buffer(ipc_buffer as u64);
    init_logger();
    info!(
        "xv6fs-server: boot protocol={} abi={} block={} first-op={}",
        Xv6Protocol::VfsToXv6Fs.raw(),
        XV6_ABI_VERSION,
        FS_BLOCK_SIZE,
        Xv6FsOp::OpenAt.raw()
    );
    info!("xv6fs-server: waiting for vfs-server and disk-server hookup");
    rt::block_on(server_loop());
    error!("xv6fs-server: server loop returned");
    halt_loop()
}

async fn server_loop() {
    let mut reply_pending = false;
    let mut reply_mrs = [0u64; 4];
    loop {
        let msg = if reply_pending {
            reply_pending = false;
            rt::reply_recv_with_reply(
                XV6_SERVICE_ENDPOINT_CPTR,
                msg_info(0, 0, 0, 4),
                &reply_mrs,
                XV6_SERVER_RECV_REPLY_CPTR,
            )
            .await
        } else {
            rt::recv_with_reply(XV6_SERVICE_ENDPOINT_CPTR, XV6_SERVER_RECV_REPLY_CPTR).await
        };
        if (msg.badge & Xv6Badge::DiskCompletion.raw()) != 0 {
            continue;
        }

        match handle_request(&msg).await {
            continuation::RequestResult::Reply(mrs) => {
                reply_mrs = mrs;
                reply_pending = true;
            }
            continuation::RequestResult::Deferred => {
                reply_pending = false;
            }
        }
    }
}

async fn handle_request(msg: &IpcMessage) -> continuation::RequestResult {
    let raw_op = msg_label(msg.info);
    match Xv6FsOp::from_raw(raw_op) {
        Some(Xv6FsOp::Init) => continuation::RequestResult::Reply(handle_init(msg)),
        Some(Xv6FsOp::OpenAt) => continuation::handle_open_at(*msg).await,
        Some(Xv6FsOp::Release) => continuation::handle_release(msg).await,
        Some(Xv6FsOp::Read) => continuation::handle_read(msg, Xv6FileType::File.raw()).await,
        Some(Xv6FsOp::Write) => continuation::handle_write(*msg).await,
        Some(Xv6FsOp::Fstat) => continuation::handle_fstat(msg).await,
        Some(Xv6FsOp::LookupDirectory) => continuation::handle_lookup_dir(msg).await,
        Some(Xv6FsOp::ReadDir) => {
            continuation::handle_read(msg, Xv6FileType::Directory.raw()).await
        }
        Some(Xv6FsOp::Retain) => continuation::handle_retain(msg).await,
        Some(Xv6FsOp::Unlink) => continuation::handle_transactional_op(*msg, handle_unlink).await,
        Some(Xv6FsOp::Link) => continuation::handle_transactional_op(*msg, handle_link).await,
        Some(Xv6FsOp::Mkdir) => continuation::handle_transactional_op(*msg, handle_mkdir).await,
        Some(Xv6FsOp::Mknod) => continuation::handle_transactional_op(*msg, handle_mknod).await,
        None => {
            warn!("xv6fs-server: unsupported op={}", raw_op);
            continuation::RequestResult::Reply([Xv6Status::NoSyscall.raw(), 0, 0, 0])
        }
    }
}

fn handle_init(msg: &IpcMessage) -> [u64; 4] {
    if msg.mrs[0] != Xv6Protocol::VfsToXv6Fs.raw() || msg.mrs[1] != XV6_ABI_VERSION {
        warn!("xv6fs-server: bad init protocol");
        return [Xv6Status::InvalidArgument.raw(), 0, 0, 0];
    }
    info!("xv6fs-server: init from vfs");
    let disk = unsafe {
        sel4_call(
            XV6_DISK_ENDPOINT_CPTR,
            msg_info(DiskRequestOp::GetInfo.raw(), 0, 0, 2),
            &[Xv6Protocol::FsToDisk.raw(), XV6_ABI_VERSION],
        )
    };
    if msg_label(disk.info) != 0 || disk.mrs[0] != Xv6Status::Ok.raw() {
        warn!("xv6fs-server: disk info failed status={}", disk.mrs[0]);
        return [disk.mrs[0], 0, 0, 0];
    }
    if disk.mrs[1] != VIRTIO_BLK_SECTOR_SIZE as u64 {
        warn!("xv6fs-server: unexpected disk sector size");
        return [Xv6Status::InvalidArgument.raw(), 0, 0, 0];
    }

    if !read_disk_block(1) {
        warn!("xv6fs-server: superblock read failed");
        return [Xv6Status::InvalidArgument.raw(), 0, 0, 0];
    }
    fence(Ordering::SeqCst);
    let superblock = read_superblock_from_shared();
    info!("xv6fs-server: superblock magic={}", superblock.magic);
    if superblock.magic != XV6_FS_MAGIC {
        warn!(
            "xv6fs-server: unexpected superblock magic={}",
            superblock.magic
        );
        return [Xv6Status::InvalidArgument.raw(), 0, 0, 0];
    }
    if superblock.size != disk.mrs[2] as u32 {
        warn!("xv6fs-server: superblock/disk block mismatch");
        return [Xv6Status::InvalidArgument.raw(), 0, 0, 0];
    }
    if superblock.nlog == 0
        || superblock.nlog as usize > XV6_LOG_MAX_BLOCKS + 1
        || superblock.logstart.saturating_add(superblock.nlog) > superblock.size
    {
        warn!(
            "xv6fs-server: unsupported log geometry nlog={}",
            superblock.nlog
        );
        return [Xv6Status::InvalidArgument.raw(), 0, 0, 0];
    }

    FS_STATE.set(FsState {
        ready: true,
        superblock,
    });
    if !recover_log() {
        warn!("xv6fs-server: log recovery failed");
        return [Xv6Status::InvalidArgument.raw(), 0, 0, 0];
    }
    if !reclaim_orphan_inodes() {
        warn!("xv6fs-server: orphan reclaim failed");
        return [Xv6Status::InvalidArgument.raw(), 0, 0, 0];
    }

    if superblock.size == 0 || !exercise_disk_write(superblock.size - 1) {
        warn!("xv6fs-server: disk write verification failed");
        return [Xv6Status::InvalidArgument.raw(), 0, 0, 0];
    }
    if !exercise_concurrent_reads(1, superblock.size - 1) {
        warn!("xv6fs-server: concurrent disk read verification failed");
        return [Xv6Status::InvalidArgument.raw(), 0, 0, 0];
    }

    let Some(root) = read_inode(XV6_FS_ROOT_INUM) else {
        warn!("xv6fs-server: root inode read failed");
        return [Xv6Status::InvalidArgument.raw(), 0, 0, 0];
    };
    if root.typ != Xv6FileType::Directory.raw() as i16 {
        warn!("xv6fs-server: root inode is not a directory");
        return [Xv6Status::InvalidArgument.raw(), 0, 0, 0];
    }
    let root_entries = count_dir_entries(&root);
    let Some((readme_ino, readme)) = lookup_root_name(b"README") else {
        warn!("xv6fs-server: README not found in fs.img root");
        return [Xv6Status::InvalidArgument.raw(), 0, 0, 0];
    };
    info!(
        "xv6fs-server: root entries={} README ino={} size={} nlink={}",
        root_entries, readme_ino, readme.size, readme.nlink
    );

    info!(
        "xv6fs-server: disk ready sector={} fs-block={} blocks={} magic={}",
        disk.mrs[1], FS_BLOCK_SIZE, disk.mrs[2], superblock.magic
    );
    [
        Xv6Status::Ok.raw(),
        disk.mrs[1],
        FS_BLOCK_SIZE as u64,
        disk.mrs[2],
    ]
}

fn reclaim_orphan_inodes() -> bool {
    let state = FS_STATE.get();
    if !state.ready {
        return false;
    }
    let mut inum = 1u32;
    while inum < state.superblock.ninodes {
        let Some(inode) = read_inode(inum) else {
            return false;
        };
        if inode.typ != 0 && inode.nlink == 0 {
            info!("ireclaim: orphaned inode {}", inum);
            if handle_transactional(|| {
                if free_inode(inum) {
                    [Xv6Status::Ok.raw(), 0, 0, 0]
                } else {
                    [Xv6Status::InvalidArgument.raw(), 0, 0, 0]
                }
            })[0]
                != Xv6Status::Ok.raw()
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
    error!("xv6fs-server: panic");
    halt_loop()
}
