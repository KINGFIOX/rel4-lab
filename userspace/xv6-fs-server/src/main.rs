#![no_std]
#![no_main]
#![deny(unsafe_attr_outside_unsafe)]
#![deny(unsafe_op_in_unsafe_fn)]

use core::panic::PanicInfo;
use core::sync::atomic::{Ordering, fence};

mod block;
mod dir;
mod inode;
mod types;

use block::{
    exercise_disk_write, handle_transactional, read_disk_block, read_superblock_from_shared,
    recover_log,
};
use dir::{
    add_dir_entry_to_inode, clear_dirent, count_dir_entries, create_node_from, find_dir_entry,
    is_dir_empty, log_bytes, lookup_parent_from, lookup_path_from, lookup_root_name,
};
use inode::{
    data_block, free_inode, inode_open_refs, read_inode, release_inode, retain_inode,
    truncate_inode, write_inode, write_inode_data, write_inode_meta,
};
use sel4_user::{
    IpcMessage, halt_loop, init_ipc_buffer, log, msg_info, msg_label, print_u64, sel4_call,
    sel4_recv, sel4_reply_recv,
};
use types::{FS_STATE, FsState, XV6_LOG_MAX_BLOCKS};
use xv6_abi::{
    DISK_OP_GET_INFO, FS_BLOCK_SIZE, FS_OP_CHDIR, FS_OP_CLOSE, FS_OP_EXEC_LOOKUP, FS_OP_FSTAT,
    FS_OP_INIT, FS_OP_LINK, FS_OP_MKDIR, FS_OP_MKNOD, FS_OP_OPEN, FS_OP_READ, FS_OP_READDIR,
    FS_OP_UNLINK, FS_OP_WRITE, O_CREATE, O_RDWR, O_TRUNC, O_WRONLY, T_DEVICE, T_DIR, T_FILE,
    VIRTIO_BLK_SECTOR_SIZE, XV6_ABI_VERSION, XV6_DISK_ENDPOINT_CPTR, XV6_DISK_SHARED_BUFFER_VADDR,
    XV6_EINVAL, XV6_ENOSYS, XV6_FS_MAGIC, XV6_FS_ROOT_INUM, XV6_FS_TO_DISK_PROTOCOL,
    XV6_HOST_TO_FS_PROTOCOL, XV6_OK, XV6_SERVICE_ENDPOINT_CPTR,
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
        FS_OP_OPEN => handle_transactional(|| handle_open(msg)),
        FS_OP_CLOSE => handle_transactional(|| handle_close(msg)),
        FS_OP_READ => handle_read(msg),
        FS_OP_WRITE => handle_transactional(|| handle_write(msg)),
        FS_OP_FSTAT => handle_fstat(msg),
        FS_OP_CHDIR => handle_chdir(msg),
        FS_OP_EXEC_LOOKUP => handle_exec_lookup(msg),
        FS_OP_READDIR => handle_readdir(msg),
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

fn handle_open(msg: &IpcMessage) -> [u64; 4] {
    if msg.mrs[0] != XV6_HOST_TO_FS_PROTOCOL || msg.mrs[1] != XV6_ABI_VERSION {
        log("xv6-fs-server: bad open protocol\n");
        return [XV6_EINVAL, 0, 0, 0];
    }
    if unsafe { !FS_STATE.ready } {
        log("xv6-fs-server: open before init\n");
        return [XV6_EINVAL, 0, 0, 0];
    }
    let cwd_inum = msg.mrs[2] as u32;
    let flags = msg.mrs[3] as u32;
    let path_len = msg.mrs[4] as usize;
    let mut path = [0u8; 128];
    if !unpack_path(&msg.mrs, 5, path_len, &mut path) {
        return [XV6_EINVAL, 0, 0, 0];
    };
    let path = &path[..path_len];
    let writable = flags & (O_WRONLY | O_RDWR) != 0;

    if let Some((inum, mut inode)) = lookup_path_from(cwd_inum, path) {
        if inode.typ == T_DIR as i16 && writable {
            return [XV6_EINVAL, 0, 0, 0];
        }
        if flags & O_TRUNC != 0 && inode.typ == T_FILE as i16 && writable {
            if !truncate_inode(&mut inode) || !write_inode(inum, &inode) {
                return [XV6_EINVAL, 0, 0, 0];
            }
        }
        retain_inode(inum);
        return [XV6_OK, inum as u64, inode.typ as u64, inode.size as u64];
    }

    if flags & O_CREATE == 0 {
        return [XV6_EINVAL, 0, 0, 0];
    }
    let Some((inum, inode)) = create_node_from(cwd_inum, path, T_FILE, 0, 0) else {
        return [XV6_EINVAL, 0, 0, 0];
    };
    retain_inode(inum);
    [XV6_OK, inum as u64, inode.typ as u64, inode.size as u64]
}

fn handle_close(msg: &IpcMessage) -> [u64; 4] {
    if msg.mrs[0] != XV6_HOST_TO_FS_PROTOCOL || msg.mrs[1] != XV6_ABI_VERSION {
        log("xv6-fs-server: bad close protocol\n");
        return [XV6_EINVAL, 0, 0, 0];
    }
    if unsafe { !FS_STATE.ready } {
        log("xv6-fs-server: close before init\n");
        return [XV6_EINVAL, 0, 0, 0];
    }
    let inum = msg.mrs[2] as u32;
    release_inode(inum);
    if let Some(inode) = read_inode(inum) {
        if inode.typ != 0 && inode.nlink == 0 && inode_open_refs(inum) == 0 {
            let _ = free_inode(inum);
        }
    }
    [XV6_OK, 0, 0, 0]
}

fn handle_read(msg: &IpcMessage) -> [u64; 4] {
    handle_read_data(msg, T_FILE)
}

fn handle_write(msg: &IpcMessage) -> [u64; 4] {
    if msg.mrs[0] != XV6_HOST_TO_FS_PROTOCOL || msg.mrs[1] != XV6_ABI_VERSION {
        log("xv6-fs-server: bad write protocol\n");
        return [XV6_EINVAL, 0, 0, 0];
    }
    if unsafe { !FS_STATE.ready } {
        log("xv6-fs-server: write before init\n");
        return [XV6_EINVAL, 0, 0, 0];
    }
    let inum = msg.mrs[2] as u32;
    let offset = msg.mrs[3] as usize;
    let len = core::cmp::min(msg.mrs[4] as usize, FS_BLOCK_SIZE);
    let mut src = [0u8; FS_BLOCK_SIZE];
    unsafe {
        core::ptr::copy_nonoverlapping(
            XV6_DISK_SHARED_BUFFER_VADDR as *const u8,
            src.as_mut_ptr(),
            len,
        );
    }
    let Some(mut inode) = read_inode(inum) else {
        return [XV6_EINVAL, 0, 0, 0];
    };
    if inode.typ != T_FILE as i16 || offset > inode.size as usize {
        return [XV6_EINVAL, 0, 0, 0];
    }
    let Some(written) = write_inode_data(inum, &mut inode, offset, &src[..len]) else {
        return [XV6_EINVAL, 0, 0, 0];
    };
    [XV6_OK, written as u64, inode.typ as u64, inode.size as u64]
}

fn handle_readdir(msg: &IpcMessage) -> [u64; 4] {
    handle_read_data(msg, T_DIR)
}

fn handle_read_data(msg: &IpcMessage, expected_type: u16) -> [u64; 4] {
    if msg.mrs[0] != XV6_HOST_TO_FS_PROTOCOL || msg.mrs[1] != XV6_ABI_VERSION {
        log("xv6-fs-server: bad read protocol\n");
        return [XV6_EINVAL, 0, 0, 0];
    }
    if unsafe { !FS_STATE.ready } {
        log("xv6-fs-server: read before init\n");
        return [XV6_EINVAL, 0, 0, 0];
    }
    let inum = msg.mrs[2] as u32;
    let offset = msg.mrs[3] as usize;
    let max_len = core::cmp::min(msg.mrs[4] as usize, FS_BLOCK_SIZE);
    let Some(inode) = read_inode(inum) else {
        return [XV6_EINVAL, 0, 0, 0];
    };
    if inode.typ != expected_type as i16 {
        return [XV6_EINVAL, 0, 0, 0];
    }
    if max_len == 0 || offset >= inode.size as usize {
        return [XV6_OK, 0, inode.typ as u64, inode.size as u64];
    }
    let file_block_index = offset / FS_BLOCK_SIZE;
    let block_offset = offset % FS_BLOCK_SIZE;
    let Some(blockno) = data_block(&inode, file_block_index) else {
        return [XV6_EINVAL, 0, 0, 0];
    };
    if !read_disk_block(blockno) {
        return [XV6_EINVAL, 0, 0, 0];
    }
    fence(Ordering::SeqCst);
    let n = core::cmp::min(
        max_len,
        core::cmp::min(inode.size as usize - offset, FS_BLOCK_SIZE - block_offset),
    );
    if block_offset != 0 && n != 0 {
        unsafe {
            core::ptr::copy(
                (XV6_DISK_SHARED_BUFFER_VADDR as *const u8).add(block_offset),
                XV6_DISK_SHARED_BUFFER_VADDR as *mut u8,
                n,
            );
        }
        fence(Ordering::SeqCst);
    }
    [XV6_OK, n as u64, inode.typ as u64, inode.size as u64]
}

fn handle_fstat(msg: &IpcMessage) -> [u64; 4] {
    if msg.mrs[0] != XV6_HOST_TO_FS_PROTOCOL || msg.mrs[1] != XV6_ABI_VERSION {
        log("xv6-fs-server: bad fstat protocol\n");
        return [XV6_EINVAL, 0, 0, 0];
    }
    if unsafe { !FS_STATE.ready } {
        log("xv6-fs-server: fstat before init\n");
        return [XV6_EINVAL, 0, 0, 0];
    }
    let Some(inode) = read_inode(msg.mrs[2] as u32) else {
        return [XV6_EINVAL, 0, 0, 0];
    };
    [
        XV6_OK,
        inode.typ as u64,
        inode.nlink as u64,
        inode.size as u64,
    ]
}

fn handle_chdir(msg: &IpcMessage) -> [u64; 4] {
    if msg.mrs[0] != XV6_HOST_TO_FS_PROTOCOL || msg.mrs[1] != XV6_ABI_VERSION {
        log("xv6-fs-server: bad chdir protocol\n");
        return [XV6_EINVAL, 0, 0, 0];
    }
    if unsafe { !FS_STATE.ready } {
        log("xv6-fs-server: chdir before init\n");
        return [XV6_EINVAL, 0, 0, 0];
    }
    let cwd_inum = msg.mrs[2] as u32;
    let path_len = msg.mrs[4] as usize;
    let mut path = [0u8; 128];
    if !unpack_path(&msg.mrs, 5, path_len, &mut path) {
        return [XV6_EINVAL, 0, 0, 0];
    };
    let Some((inum, inode)) = lookup_path_from(cwd_inum, &path[..path_len]) else {
        return [XV6_EINVAL, 0, 0, 0];
    };
    if inode.typ != T_DIR as i16 {
        return [XV6_EINVAL, 0, 0, 0];
    }
    [XV6_OK, inum as u64, inode.size as u64, 0]
}

fn handle_exec_lookup(msg: &IpcMessage) -> [u64; 4] {
    if msg.mrs[0] != XV6_HOST_TO_FS_PROTOCOL || msg.mrs[1] != XV6_ABI_VERSION {
        log("xv6-fs-server: bad exec lookup protocol\n");
        return [XV6_EINVAL, 0, 0, 0];
    }
    if unsafe { !FS_STATE.ready } {
        log("xv6-fs-server: exec lookup before init\n");
        return [XV6_EINVAL, 0, 0, 0];
    }
    let cwd_inum = msg.mrs[2] as u32;
    let path_len = msg.mrs[4] as usize;
    let mut path = [0u8; 128];
    if !unpack_path(&msg.mrs, 5, path_len, &mut path) {
        return [XV6_EINVAL, 0, 0, 0];
    };
    let Some((inum, inode)) = lookup_path_from(cwd_inum, &path[..path_len]) else {
        return [XV6_EINVAL, 0, 0, 0];
    };
    if inode.typ != T_FILE as i16 {
        return [XV6_EINVAL, 0, 0, 0];
    }
    [XV6_OK, inum as u64, inode.size as u64, 0]
}

fn handle_unlink(msg: &IpcMessage) -> [u64; 4] {
    if msg.mrs[0] != XV6_HOST_TO_FS_PROTOCOL || msg.mrs[1] != XV6_ABI_VERSION {
        log("xv6-fs-server: bad unlink protocol\n");
        return [XV6_EINVAL, 0, 0, 0];
    }
    if unsafe { !FS_STATE.ready } {
        log("xv6-fs-server: unlink before init\n");
        return [XV6_EINVAL, 0, 0, 0];
    }
    let cwd_inum = msg.mrs[2] as u32;
    let path_len = msg.mrs[3] as usize;
    let mut path = [0u8; 128];
    if !unpack_path(&msg.mrs, 4, path_len, &mut path) {
        return [XV6_EINVAL, 0, 0, 0];
    }
    let Some((parent_inum, mut parent, name, name_len)) =
        lookup_parent_from(cwd_inum, &path[..path_len], true)
    else {
        return [XV6_EINVAL, 0, 0, 0];
    };
    let name = &name[..name_len];
    let Some(loc) = find_dir_entry(&parent, name) else {
        return [XV6_EINVAL, 0, 0, 0];
    };
    let Some(mut inode) = read_inode(loc.inum) else {
        return [XV6_EINVAL, 0, 0, 0];
    };
    if inode.typ == T_DIR as i16 && !is_dir_empty(&inode) {
        return [XV6_EINVAL, 0, 0, 0];
    }
    if inode.nlink == 0 {
        return [XV6_EINVAL, 0, 0, 0];
    }
    if !clear_dirent(loc) {
        return [XV6_EINVAL, 0, 0, 0];
    }
    if inode.typ == T_DIR as i16 {
        if parent.nlink == 0 {
            return [XV6_EINVAL, 0, 0, 0];
        }
        parent.nlink -= 1;
        if !write_inode(parent_inum, &parent) {
            return [XV6_EINVAL, 0, 0, 0];
        }
    }
    inode.nlink -= 1;
    if !write_inode_meta(loc.inum, &inode) {
        return [XV6_EINVAL, 0, 0, 0];
    }
    if inode.nlink == 0 && inode_open_refs(loc.inum) == 0 {
        let _ = free_inode(loc.inum);
    }
    log("xv6-fs-server: unlink ");
    log_bytes(name);
    log(" ino=");
    print_u64(loc.inum as u64);
    log(" nlink=");
    print_u64(inode.nlink as u64);
    log("\n");
    [XV6_OK, loc.inum as u64, inode.nlink as u64, 0]
}

fn handle_link(msg: &IpcMessage) -> [u64; 4] {
    if msg.mrs[0] != XV6_HOST_TO_FS_PROTOCOL || msg.mrs[1] != XV6_ABI_VERSION {
        log("xv6-fs-server: bad link protocol\n");
        return [XV6_EINVAL, 0, 0, 0];
    }
    if unsafe { !FS_STATE.ready } {
        log("xv6-fs-server: link before init\n");
        return [XV6_EINVAL, 0, 0, 0];
    }
    let cwd_inum = msg.mrs[2] as u32;
    let old_len = msg.mrs[3] as usize;
    let new_len = msg.mrs[4] as usize;
    let old_words = old_len.div_ceil(8);
    let mut old_path = [0u8; 128];
    let mut new_path = [0u8; 128];
    if !unpack_path(&msg.mrs, 5, old_len, &mut old_path)
        || !unpack_path(&msg.mrs, 5 + old_words, new_len, &mut new_path)
    {
        return [XV6_EINVAL, 0, 0, 0];
    }
    let Some((old_inum, mut inode)) = lookup_path_from(cwd_inum, &old_path[..old_len]) else {
        return [XV6_EINVAL, 0, 0, 0];
    };
    if inode.typ != T_FILE as i16 || inode.nlink == u16::MAX {
        return [XV6_EINVAL, 0, 0, 0];
    }
    let Some((parent_inum, mut parent, new_name, new_name_len)) =
        lookup_parent_from(cwd_inum, &new_path[..new_len], true)
    else {
        return [XV6_EINVAL, 0, 0, 0];
    };
    let new_name = &new_name[..new_name_len];
    if find_dir_entry(&parent, new_name).is_some() {
        return [XV6_EINVAL, 0, 0, 0];
    }

    inode.nlink += 1;
    if !write_inode(old_inum, &inode) {
        return [XV6_EINVAL, 0, 0, 0];
    }
    if !add_dir_entry_to_inode(parent_inum, &mut parent, new_name, old_inum) {
        inode.nlink -= 1;
        let _ = write_inode(old_inum, &inode);
        return [XV6_EINVAL, 0, 0, 0];
    }
    log("xv6-fs-server: link ");
    log_bytes(&old_path[..old_len]);
    log(" -> ");
    log_bytes(new_name);
    log(" ino=");
    print_u64(old_inum as u64);
    log(" nlink=");
    print_u64(inode.nlink as u64);
    log("\n");
    [XV6_OK, old_inum as u64, inode.nlink as u64, 0]
}

fn handle_mkdir(msg: &IpcMessage) -> [u64; 4] {
    if msg.mrs[0] != XV6_HOST_TO_FS_PROTOCOL || msg.mrs[1] != XV6_ABI_VERSION {
        log("xv6-fs-server: bad mkdir protocol\n");
        return [XV6_EINVAL, 0, 0, 0];
    }
    if unsafe { !FS_STATE.ready } {
        log("xv6-fs-server: mkdir before init\n");
        return [XV6_EINVAL, 0, 0, 0];
    }
    let cwd_inum = msg.mrs[2] as u32;
    let path_len = msg.mrs[3] as usize;
    let mut path = [0u8; 128];
    if !unpack_path(&msg.mrs, 4, path_len, &mut path) {
        return [XV6_EINVAL, 0, 0, 0];
    }
    let Some((inum, inode)) = create_node_from(cwd_inum, &path[..path_len], T_DIR, 0, 0) else {
        return [XV6_EINVAL, 0, 0, 0];
    };
    [XV6_OK, inum as u64, inode.nlink as u64, inode.size as u64]
}

fn handle_mknod(msg: &IpcMessage) -> [u64; 4] {
    if msg.mrs[0] != XV6_HOST_TO_FS_PROTOCOL || msg.mrs[1] != XV6_ABI_VERSION {
        log("xv6-fs-server: bad mknod protocol\n");
        return [XV6_EINVAL, 0, 0, 0];
    }
    if unsafe { !FS_STATE.ready } {
        log("xv6-fs-server: mknod before init\n");
        return [XV6_EINVAL, 0, 0, 0];
    }
    let cwd_inum = msg.mrs[2] as u32;
    let major = msg.mrs[3] as u16;
    let minor = msg.mrs[4] as u16;
    let path_len = msg.mrs[5] as usize;
    let mut path = [0u8; 128];
    if !unpack_path(&msg.mrs, 6, path_len, &mut path) {
        return [XV6_EINVAL, 0, 0, 0];
    }
    let Some((inum, inode)) = create_node_from(cwd_inum, &path[..path_len], T_DEVICE, major, minor)
    else {
        return [XV6_EINVAL, 0, 0, 0];
    };
    [XV6_OK, inum as u64, inode.major as u64, inode.minor as u64]
}

fn unpack_path(mrs: &[u64; 64], start: usize, path_len: usize, out: &mut [u8; 128]) -> bool {
    if path_len == 0 || path_len > out.len() || start + path_len.div_ceil(8) > mrs.len() {
        return false;
    }
    let mut i = 0usize;
    while i < path_len {
        let word = mrs[start + i / 8];
        out[i] = ((word >> ((i % 8) * 8)) & 0xff) as u8;
        i += 1;
    }
    true
}

#[panic_handler]
fn panic(_: &PanicInfo) -> ! {
    log("xv6-fs-server: panic\n");
    halt_loop()
}
