#![no_std]
#![no_main]
#![deny(unsafe_attr_outside_unsafe)]
#![deny(unsafe_op_in_unsafe_fn)]

use core::panic::PanicInfo;
use core::sync::atomic::{Ordering, fence};

mod block;
mod types;

use block::{
    exercise_disk_write, handle_transactional, read_disk_block, read_superblock_from_shared,
    recover_log, shared_block, shared_block_mut, write_disk_block,
};
use sel4_user::{
    IpcMessage, halt_loop, init_ipc_buffer, log, msg_info, msg_label, print_u64, read_u16,
    read_u32, sel4_call, sel4_recv, sel4_reply_recv, write_u16, write_u32,
};
use types::{
    BPB, DINODE_SIZE, DINODES_PER_BLOCK, Dinode, DirEntryLoc, FS_MAX_TRACKED_INODES, FS_STATE,
    FsState, OPEN_REFS, XV6_LOG_MAX_BLOCKS,
};
use xv6_abi::{
    DIRENT_SIZE, DIRSIZ, DISK_OP_GET_INFO, FS_BLOCK_SIZE, FS_OP_CHDIR, FS_OP_CLOSE,
    FS_OP_EXEC_LOOKUP, FS_OP_FSTAT, FS_OP_INIT, FS_OP_LINK, FS_OP_MKDIR, FS_OP_MKNOD, FS_OP_OPEN,
    FS_OP_READ, FS_OP_READDIR, FS_OP_UNLINK, FS_OP_WRITE, O_CREATE, O_RDWR, O_TRUNC, O_WRONLY,
    T_DEVICE, T_DIR, T_FILE, VIRTIO_BLK_SECTOR_SIZE, XV6_ABI_VERSION, XV6_DISK_ENDPOINT_CPTR,
    XV6_DISK_SHARED_BUFFER_VADDR, XV6_EINVAL, XV6_ENOSYS, XV6_FS_MAGIC, XV6_FS_NDIRECT,
    XV6_FS_NINDIRECT, XV6_FS_ROOT_INUM, XV6_FS_TO_DISK_PROTOCOL, XV6_HOST_TO_FS_PROTOCOL, XV6_OK,
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

fn read_inode(inum: u32) -> Option<Dinode> {
    let state = unsafe { FS_STATE };
    if !state.ready || inum >= state.superblock.ninodes || inum == 0 {
        return None;
    }
    let blockno = inum / DINODES_PER_BLOCK + state.superblock.inodestart;
    let offset = ((inum % DINODES_PER_BLOCK) as usize) * DINODE_SIZE;
    if !read_disk_block(blockno) {
        return None;
    }
    fence(Ordering::SeqCst);
    let block = shared_block();
    let mut addrs = [0u32; XV6_FS_NDIRECT + 1];
    let mut i = 0;
    while i < addrs.len() {
        addrs[i] = read_u32(block, offset + 12 + i * 4);
        i += 1;
    }
    Some(Dinode {
        typ: read_u16(block, offset) as i16,
        major: read_u16(block, offset + 2),
        minor: read_u16(block, offset + 4),
        nlink: read_u16(block, offset + 6),
        size: read_u32(block, offset + 8),
        addrs,
    })
}

fn write_inode(inum: u32, inode: &Dinode) -> bool {
    let state = unsafe { FS_STATE };
    if !state.ready || inum >= state.superblock.ninodes || inum == 0 {
        return false;
    }
    let blockno = inum / DINODES_PER_BLOCK + state.superblock.inodestart;
    let offset = ((inum % DINODES_PER_BLOCK) as usize) * DINODE_SIZE;
    if !read_disk_block(blockno) {
        return false;
    }
    fence(Ordering::SeqCst);
    {
        let block = shared_block_mut();
        write_u16(block, offset, inode.typ as u16);
        write_u16(block, offset + 2, inode.major);
        write_u16(block, offset + 4, inode.minor);
        write_u16(block, offset + 6, inode.nlink);
        write_u32(block, offset + 8, inode.size);
        let mut i = 0usize;
        while i < inode.addrs.len() {
            write_u32(block, offset + 12 + i * 4, inode.addrs[i]);
            i += 1;
        }
    }
    fence(Ordering::SeqCst);
    write_disk_block(blockno)
}

fn write_inode_meta(inum: u32, inode: &Dinode) -> bool {
    let state = unsafe { FS_STATE };
    if !state.ready || inum >= state.superblock.ninodes || inum == 0 {
        return false;
    }
    let blockno = inum / DINODES_PER_BLOCK + state.superblock.inodestart;
    let offset = ((inum % DINODES_PER_BLOCK) as usize) * DINODE_SIZE;
    if !read_disk_block(blockno) {
        return false;
    }
    fence(Ordering::SeqCst);
    {
        let block = shared_block_mut();
        write_u16(block, offset + 6, inode.nlink);
        write_u32(block, offset + 8, inode.size);
    }
    fence(Ordering::SeqCst);
    write_disk_block(blockno)
}

fn inode_ref_index(inum: u32) -> Option<usize> {
    let index = inum as usize;
    (index < FS_MAX_TRACKED_INODES).then_some(index)
}

fn retain_inode(inum: u32) {
    if let Some(index) = inode_ref_index(inum) {
        unsafe {
            OPEN_REFS[index] = OPEN_REFS[index].saturating_add(1);
        }
    }
}

fn release_inode(inum: u32) {
    if let Some(index) = inode_ref_index(inum) {
        unsafe {
            if OPEN_REFS[index] != 0 {
                OPEN_REFS[index] -= 1;
            }
        }
    }
}

fn inode_open_refs(inum: u32) -> u16 {
    if let Some(index) = inode_ref_index(inum) {
        unsafe { OPEN_REFS[index] }
    } else {
        0
    }
}

fn alloc_inode_with(typ: u16, major: u16, minor: u16) -> Option<u32> {
    let state = unsafe { FS_STATE };
    if !state.ready {
        return None;
    }
    let mut inum = 1u32;
    while inum < state.superblock.ninodes {
        let inode = read_inode(inum)?;
        if inode.typ == 0 {
            let inode = Dinode {
                typ: typ as i16,
                major,
                minor,
                nlink: 1,
                size: 0,
                addrs: [0; XV6_FS_NDIRECT + 1],
            };
            return write_inode(inum, &inode).then_some(inum);
        }
        inum += 1;
    }
    None
}

fn free_inode(inum: u32) -> bool {
    let Some(mut inode) = read_inode(inum) else {
        return false;
    };
    if !truncate_inode(&mut inode) {
        return false;
    }
    inode.typ = 0;
    inode.major = 0;
    inode.minor = 0;
    inode.nlink = 0;
    inode.size = 0;
    write_inode(inum, &inode)
}

fn truncate_inode(inode: &mut Dinode) -> bool {
    let mut i = 0usize;
    while i < XV6_FS_NDIRECT {
        let blockno = inode.addrs[i];
        if blockno != 0 && !free_block(blockno) {
            return false;
        }
        inode.addrs[i] = 0;
        i += 1;
    }

    let indirect = inode.addrs[XV6_FS_NDIRECT];
    if indirect != 0 {
        if !read_disk_block(indirect) {
            return false;
        }
        fence(Ordering::SeqCst);
        let mut indirect_block = [0u8; FS_BLOCK_SIZE];
        unsafe {
            core::ptr::copy_nonoverlapping(
                XV6_DISK_SHARED_BUFFER_VADDR as *const u8,
                indirect_block.as_mut_ptr(),
                FS_BLOCK_SIZE,
            );
        }
        i = 0;
        while i < XV6_FS_NINDIRECT {
            let blockno = read_u32(&indirect_block, i * 4);
            if blockno != 0 && !free_block(blockno) {
                return false;
            }
            i += 1;
        }
        if !free_block(indirect) {
            return false;
        }
        inode.addrs[XV6_FS_NDIRECT] = 0;
    }
    inode.size = 0;
    true
}

fn write_inode_data(inum: u32, inode: &mut Dinode, offset: usize, src: &[u8]) -> Option<usize> {
    if offset > inode.size as usize {
        return None;
    }
    let max_bytes = (XV6_FS_NDIRECT + XV6_FS_NINDIRECT) * FS_BLOCK_SIZE;
    if offset >= max_bytes || src.is_empty() {
        return Some(0);
    }
    let total = core::cmp::min(src.len(), max_bytes - offset);
    let mut written = 0usize;
    while written < total {
        let current = offset + written;
        let file_block_index = current / FS_BLOCK_SIZE;
        let block_offset = current % FS_BLOCK_SIZE;
        let n = core::cmp::min(total - written, FS_BLOCK_SIZE - block_offset);
        let Some(blockno) = bmap_alloc(inode, file_block_index) else {
            break;
        };
        if !read_disk_block(blockno) {
            break;
        }
        fence(Ordering::SeqCst);
        {
            let block = shared_block_mut();
            let mut i = 0usize;
            while i < n {
                block[block_offset + i] = src[written + i];
                i += 1;
            }
        }
        fence(Ordering::SeqCst);
        if !write_disk_block(blockno) {
            break;
        }
        written += n;
    }
    if written == 0 && total != 0 {
        return None;
    }
    let end = offset.saturating_add(written);
    if end > inode.size as usize {
        inode.size = end as u32;
    }
    write_inode(inum, inode).then_some(written)
}

fn bmap_alloc(inode: &mut Dinode, file_block_index: usize) -> Option<u32> {
    if file_block_index < XV6_FS_NDIRECT {
        if inode.addrs[file_block_index] == 0 {
            inode.addrs[file_block_index] = alloc_block()?;
        }
        return Some(inode.addrs[file_block_index]);
    }

    let indirect_index = file_block_index - XV6_FS_NDIRECT;
    if indirect_index >= XV6_FS_NINDIRECT {
        return None;
    }
    if inode.addrs[XV6_FS_NDIRECT] == 0 {
        inode.addrs[XV6_FS_NDIRECT] = alloc_block()?;
    }
    let indirect = inode.addrs[XV6_FS_NDIRECT];
    if !read_disk_block(indirect) {
        return None;
    }
    fence(Ordering::SeqCst);
    let existing = read_u32(shared_block(), indirect_index * 4);
    if existing != 0 {
        return Some(existing);
    }

    let new_block = alloc_block()?;
    if !read_disk_block(indirect) {
        let _ = free_block(new_block);
        return None;
    }
    fence(Ordering::SeqCst);
    write_u32(shared_block_mut(), indirect_index * 4, new_block);
    fence(Ordering::SeqCst);
    if !write_disk_block(indirect) {
        let _ = free_block(new_block);
        return None;
    }
    Some(new_block)
}

fn alloc_block() -> Option<u32> {
    let state = unsafe { FS_STATE };
    if !state.ready {
        return None;
    }
    let mut base = 0u32;
    while base < state.superblock.size {
        let bitmap = bitmap_blockno(base);
        if !read_disk_block(bitmap) {
            return None;
        }
        fence(Ordering::SeqCst);
        let mut bit = 0u32;
        while bit < BPB && base + bit < state.superblock.size {
            let byte = (bit / 8) as usize;
            let mask = 1u8 << ((bit % 8) as u32);
            if shared_block()[byte] & mask == 0 {
                {
                    let block = shared_block_mut();
                    block[byte] |= mask;
                }
                fence(Ordering::SeqCst);
                if !write_disk_block(bitmap) {
                    return None;
                }
                let blockno = base + bit;
                return zero_block(blockno).then_some(blockno);
            }
            bit += 1;
        }
        base += BPB;
    }
    None
}

fn free_block(blockno: u32) -> bool {
    let state = unsafe { FS_STATE };
    if !state.ready || blockno >= state.superblock.size {
        return false;
    }
    let bitmap = bitmap_blockno(blockno);
    if !read_disk_block(bitmap) {
        return false;
    }
    fence(Ordering::SeqCst);
    let bit = blockno % BPB;
    let byte = (bit / 8) as usize;
    let mask = 1u8 << ((bit % 8) as u32);
    {
        let block = shared_block_mut();
        block[byte] &= !mask;
    }
    fence(Ordering::SeqCst);
    write_disk_block(bitmap)
}

fn zero_block(blockno: u32) -> bool {
    {
        let block = shared_block_mut();
        let mut i = 0usize;
        while i < FS_BLOCK_SIZE {
            block[i] = 0;
            i += 1;
        }
    }
    fence(Ordering::SeqCst);
    write_disk_block(blockno)
}

fn bitmap_blockno(blockno: u32) -> u32 {
    let state = unsafe { FS_STATE };
    blockno / BPB + state.superblock.bmapstart
}

fn count_dir_entries(inode: &Dinode) -> usize {
    let mut count = 0usize;
    let total = inode.size as usize / DIRENT_SIZE;
    let entries_per_block = FS_BLOCK_SIZE / DIRENT_SIZE;
    let mut block_index = 0usize;
    while block_index * entries_per_block < total {
        let Some(blockno) = data_block(inode, block_index) else {
            return count;
        };
        if !read_disk_block(blockno) {
            return count;
        }
        fence(Ordering::SeqCst);
        let entries_left = total - block_index * entries_per_block;
        let entries = core::cmp::min(entries_left, entries_per_block);
        let block = shared_block();
        let mut entry = 0usize;
        while entry < entries {
            let inum = read_u16(block, entry * DIRENT_SIZE);
            if inum != 0 {
                count += 1;
            }
            entry += 1;
        }
        block_index += 1;
    }
    count
}

fn lookup_root_name(name: &[u8]) -> Option<(u32, Dinode)> {
    let root = read_inode(XV6_FS_ROOT_INUM)?;
    lookup_dir_name(&root, name).and_then(|inum| read_inode(inum).map(|inode| (inum, inode)))
}

fn lookup_dir_name(dir: &Dinode, name: &[u8]) -> Option<u32> {
    find_dir_entry(dir, name).map(|loc| loc.inum)
}

fn find_dir_entry(dir: &Dinode, name: &[u8]) -> Option<DirEntryLoc> {
    if name.len() > DIRSIZ {
        return None;
    }
    let total = dir.size as usize / DIRENT_SIZE;
    let entries_per_block = FS_BLOCK_SIZE / DIRENT_SIZE;
    let mut block_index = 0usize;
    while block_index * entries_per_block < total {
        let blockno = data_block(dir, block_index)?;
        if !read_disk_block(blockno) {
            return None;
        }
        fence(Ordering::SeqCst);
        let entries_left = total - block_index * entries_per_block;
        let entries = core::cmp::min(entries_left, entries_per_block);
        let block = shared_block();
        let mut entry = 0usize;
        while entry < entries {
            let off = entry * DIRENT_SIZE;
            let inum = read_u16(block, off);
            let name_len = dirent_name_len(block, off + 2);
            if inum != 0 && name_len == name.len() && dirent_name_eq(name, off + 2) {
                return Some(DirEntryLoc {
                    inum: inum as u32,
                    blockno,
                    offset: off,
                });
            }
            entry += 1;
        }
        block_index += 1;
    }
    None
}

fn find_empty_or_append_dir_entry(dir: &mut Dinode) -> Option<(DirEntryLoc, bool)> {
    let total = dir.size as usize / DIRENT_SIZE;
    let entries_per_block = FS_BLOCK_SIZE / DIRENT_SIZE;
    let mut block_index = 0usize;
    while block_index * entries_per_block < total {
        let blockno = data_block(dir, block_index)?;
        if !read_disk_block(blockno) {
            return None;
        }
        fence(Ordering::SeqCst);
        let entries_left = total - block_index * entries_per_block;
        let entries = core::cmp::min(entries_left, entries_per_block);
        let block = shared_block();
        let mut entry = 0usize;
        while entry < entries {
            let off = entry * DIRENT_SIZE;
            if read_u16(block, off) == 0 {
                return Some((
                    DirEntryLoc {
                        inum: 0,
                        blockno,
                        offset: off,
                    },
                    false,
                ));
            }
            entry += 1;
        }
        block_index += 1;
    }

    let offset = dir.size as usize;
    if offset % FS_BLOCK_SIZE + DIRENT_SIZE > FS_BLOCK_SIZE {
        return None;
    }
    let block_index = offset / FS_BLOCK_SIZE;
    let blockno = bmap_alloc(dir, block_index)?;
    Some((
        DirEntryLoc {
            inum: 0,
            blockno,
            offset: offset % FS_BLOCK_SIZE,
        },
        true,
    ))
}

fn add_dir_entry_to_inode(dir_inum: u32, dir: &mut Dinode, name: &[u8], target_inum: u32) -> bool {
    if dir.typ != T_DIR as i16 || find_dir_entry(dir, name).is_some() {
        return false;
    }
    let Some((loc, appends)) = find_empty_or_append_dir_entry(dir) else {
        return false;
    };
    if !write_dirent(loc, target_inum, name) {
        return false;
    }
    if appends {
        dir.size = dir.size.saturating_add(DIRENT_SIZE as u32);
    }
    write_inode(dir_inum, dir)
}

fn write_dirent(loc: DirEntryLoc, inum: u32, name: &[u8]) -> bool {
    if name.len() > DIRSIZ || inum > u16::MAX as u32 || !read_disk_block(loc.blockno) {
        return false;
    }
    fence(Ordering::SeqCst);
    {
        let block = shared_block_mut();
        write_u16(block, loc.offset, inum as u16);
        let mut i = 0usize;
        while i < DIRSIZ {
            block[loc.offset + 2 + i] = 0;
            i += 1;
        }
        i = 0;
        while i < name.len() {
            block[loc.offset + 2 + i] = name[i];
            i += 1;
        }
    }
    fence(Ordering::SeqCst);
    write_disk_block(loc.blockno)
}

fn is_dir_empty(dir: &Dinode) -> bool {
    if dir.typ != T_DIR as i16 {
        return false;
    }
    let total = dir.size as usize / DIRENT_SIZE;
    let entries_per_block = FS_BLOCK_SIZE / DIRENT_SIZE;
    let mut block_index = 0usize;
    while block_index * entries_per_block < total {
        let Some(blockno) = data_block(dir, block_index) else {
            return false;
        };
        if !read_disk_block(blockno) {
            return false;
        }
        fence(Ordering::SeqCst);
        let entries_left = total - block_index * entries_per_block;
        let entries = core::cmp::min(entries_left, entries_per_block);
        let block = shared_block();
        let mut entry = 0usize;
        while entry < entries {
            let off = entry * DIRENT_SIZE;
            let inum = read_u16(block, off);
            if inum != 0 {
                let name_len = dirent_name_len(block, off + 2);
                let dot = name_len == 1 && block[off + 2] == b'.';
                let dotdot = name_len == 2 && block[off + 2] == b'.' && block[off + 3] == b'.';
                if !dot && !dotdot {
                    return false;
                }
            }
            entry += 1;
        }
        block_index += 1;
    }
    true
}

fn clear_dirent(loc: DirEntryLoc) -> bool {
    if !read_disk_block(loc.blockno) {
        return false;
    }
    fence(Ordering::SeqCst);
    {
        let block = shared_block_mut();
        let mut i = 0usize;
        while i < DIRENT_SIZE {
            block[loc.offset + i] = 0;
            i += 1;
        }
    }
    fence(Ordering::SeqCst);
    write_disk_block(loc.blockno)
}

fn dirent_name_len(block: &[u8], name_offset: usize) -> usize {
    let mut name_len = 0usize;
    while name_len < DIRSIZ && block[name_offset + name_len] != 0 {
        name_len += 1;
    }
    name_len
}

fn dirent_name_eq(name: &[u8], name_offset: usize) -> bool {
    let mut i = 0usize;
    while i < name.len() {
        if shared_block()[name_offset + i] != name[i] {
            return false;
        }
        i += 1;
    }
    true
}

fn data_block(inode: &Dinode, file_block_index: usize) -> Option<u32> {
    if file_block_index < XV6_FS_NDIRECT {
        let blockno = inode.addrs[file_block_index];
        return (blockno != 0).then_some(blockno);
    }
    let indirect_index = file_block_index - XV6_FS_NDIRECT;
    if indirect_index < XV6_FS_NINDIRECT {
        let indirect = inode.addrs[XV6_FS_NDIRECT];
        if indirect == 0 || !read_disk_block(indirect) {
            return None;
        }
        fence(Ordering::SeqCst);
        let blockno = read_u32(shared_block(), indirect_index * 4);
        return (blockno != 0).then_some(blockno);
    }
    None
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

fn lookup_path_from(cwd_inum: u32, path: &[u8]) -> Option<(u32, Dinode)> {
    let mut cur_inum = if path.first() == Some(&b'/') {
        XV6_FS_ROOT_INUM
    } else {
        cwd_inum
    };
    let mut cur = read_inode(cur_inum)?;
    let mut pos = 0usize;
    loop {
        while pos < path.len() && path[pos] == b'/' {
            pos += 1;
        }
        if pos >= path.len() {
            return Some((cur_inum, cur));
        }
        let start = pos;
        while pos < path.len() && path[pos] != b'/' {
            pos += 1;
        }
        let component = &path[start..pos];
        if component == b"." {
            continue;
        }
        if cur.typ != T_DIR as i16 {
            return None;
        }
        let (name, name_len) = dir_component_name(component)?;
        let loc = find_dir_entry(&cur, &name[..name_len])?;
        cur_inum = loc.inum;
        cur = read_inode(cur_inum)?;
    }
}

fn lookup_parent_from(
    cwd_inum: u32,
    path: &[u8],
    reject_dot: bool,
) -> Option<(u32, Dinode, [u8; DIRSIZ], usize)> {
    let mut end = path.len();
    while end > 0 && path[end - 1] == b'/' {
        end -= 1;
    }
    if end == 0 {
        return None;
    }
    let mut slash = end;
    while slash > 0 && path[slash - 1] != b'/' {
        slash -= 1;
    }
    let component = &path[slash..end];
    if reject_dot && (component == b"." || component == b"..") {
        return None;
    }
    let (name, name_len) = dir_component_name(component)?;
    let parent = if slash == 0 {
        let parent_inum = if path.first() == Some(&b'/') {
            XV6_FS_ROOT_INUM
        } else {
            cwd_inum
        };
        read_inode(parent_inum).map(|inode| (parent_inum, inode))?
    } else {
        lookup_path_from(cwd_inum, &path[..slash])?
    };
    if parent.1.typ != T_DIR as i16 {
        return None;
    }
    Some((parent.0, parent.1, name, name_len))
}

fn dir_component_name(component: &[u8]) -> Option<([u8; DIRSIZ], usize)> {
    if component.is_empty() {
        return None;
    }
    let mut name = [0u8; DIRSIZ];
    let name_len = core::cmp::min(component.len(), DIRSIZ);
    let mut i = 0usize;
    while i < name_len {
        name[i] = component[i];
        i += 1;
    }
    Some((name, name_len))
}

fn create_node_from(
    cwd_inum: u32,
    path: &[u8],
    typ: u16,
    major: u16,
    minor: u16,
) -> Option<(u32, Dinode)> {
    let (parent_inum, mut parent, name, name_len) = lookup_parent_from(cwd_inum, path, true)?;
    let name = &name[..name_len];
    if let Some(existing) = find_dir_entry(&parent, name) {
        let existing_inode = read_inode(existing.inum)?;
        if typ == T_FILE
            && (existing_inode.typ == T_FILE as i16 || existing_inode.typ == T_DEVICE as i16)
        {
            return Some((existing.inum, existing_inode));
        }
        return None;
    }

    let inum = alloc_inode_with(typ, major, minor)?;
    let mut inode = read_inode(inum)?;
    if typ == T_DIR {
        if !add_dir_entry_to_inode(inum, &mut inode, b".", inum)
            || !add_dir_entry_to_inode(inum, &mut inode, b"..", parent_inum)
        {
            let _ = free_inode(inum);
            return None;
        }
    }
    if !add_dir_entry_to_inode(parent_inum, &mut parent, name, inum) {
        let _ = free_inode(inum);
        return None;
    }
    if typ == T_DIR {
        parent.nlink = parent.nlink.saturating_add(1);
        if !write_inode(parent_inum, &parent) {
            let _ = free_inode(inum);
            return None;
        }
    }
    inode = read_inode(inum)?;
    log("xv6-fs-server: create ");
    log_bytes(path);
    log(" ino=");
    print_u64(inum as u64);
    log(" type=");
    print_u64(typ as u64);
    log("\n");
    Some((inum, inode))
}

fn log_bytes(bytes: &[u8]) {
    let mut i = 0usize;
    while i < bytes.len() {
        sel4_user::putchar(bytes[i]);
        i += 1;
    }
}

#[panic_handler]
fn panic(_: &PanicInfo) -> ! {
    log("xv6-fs-server: panic\n");
    halt_loop()
}
