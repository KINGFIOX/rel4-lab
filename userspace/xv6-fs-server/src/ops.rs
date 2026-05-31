use core::sync::atomic::{Ordering, fence};

use sel4_user::{IpcMessage, log, print_u64};
use xv6_abi::{
    FS_BLOCK_SIZE, O_CREATE, O_RDWR, O_TRUNC, O_WRONLY, T_DEVICE, T_DIR, T_FILE, XV6_ABI_VERSION,
    XV6_EINVAL, XV6_HOST_TO_FS_PROTOCOL, XV6_MAX_FILE_WRITE, XV6_OK,
};

use crate::block::{
    host_shared_block_mut, host_shared_write_buffer, read_disk_block, shared_block,
};
use crate::dir::{
    add_dir_entry_to_inode, clear_dirent, create_node_from, find_dir_entry, is_dir_empty,
    log_bytes, lookup_parent_from, lookup_path_from,
};
use crate::inode::{
    data_block, free_inode, inode_open_refs, read_inode, release_inode, retain_inode,
    truncate_inode, write_inode, write_inode_data, write_inode_meta,
};
use crate::types::FS_STATE;

pub(crate) fn handle_open(msg: &IpcMessage) -> [u64; 4] {
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

pub(crate) fn handle_close(msg: &IpcMessage) -> [u64; 4] {
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

pub(crate) fn handle_read(msg: &IpcMessage) -> [u64; 4] {
    handle_read_data(msg, T_FILE)
}

pub(crate) fn handle_write(msg: &IpcMessage) -> [u64; 4] {
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
    let len = core::cmp::min(msg.mrs[4] as usize, XV6_MAX_FILE_WRITE);
    let src = &host_shared_write_buffer()[..len];
    let Some(mut inode) = read_inode(inum) else {
        return [XV6_EINVAL, 0, 0, 0];
    };
    if inode.typ != T_FILE as i16 || offset > inode.size as usize {
        return [XV6_EINVAL, 0, 0, 0];
    }
    let Some(written) = write_inode_data(inum, &mut inode, offset, src) else {
        return [XV6_EINVAL, 0, 0, 0];
    };
    [XV6_OK, written as u64, inode.typ as u64, inode.size as u64]
}

pub(crate) fn handle_readdir(msg: &IpcMessage) -> [u64; 4] {
    handle_read_data(msg, T_DIR)
}

pub(crate) fn handle_retain(msg: &IpcMessage) -> [u64; 4] {
    if msg.mrs[0] != XV6_HOST_TO_FS_PROTOCOL || msg.mrs[1] != XV6_ABI_VERSION {
        log("xv6-fs-server: bad retain protocol\n");
        return [XV6_EINVAL, 0, 0, 0];
    }
    if unsafe { !FS_STATE.ready } {
        log("xv6-fs-server: retain before init\n");
        return [XV6_EINVAL, 0, 0, 0];
    }
    let inum = msg.mrs[2] as u32;
    let Some(inode) = read_inode(inum) else {
        return [XV6_EINVAL, 0, 0, 0];
    };
    if inode.typ == 0 {
        return [XV6_EINVAL, 0, 0, 0];
    }
    retain_inode(inum);
    [XV6_OK, inum as u64, inode.nlink as u64, inode.size as u64]
}

pub(crate) fn handle_fstat(msg: &IpcMessage) -> [u64; 4] {
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

pub(crate) fn handle_chdir(msg: &IpcMessage) -> [u64; 4] {
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

pub(crate) fn handle_exec_lookup(msg: &IpcMessage) -> [u64; 4] {
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

pub(crate) fn handle_unlink(msg: &IpcMessage) -> [u64; 4] {
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

pub(crate) fn handle_link(msg: &IpcMessage) -> [u64; 4] {
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

pub(crate) fn handle_mkdir(msg: &IpcMessage) -> [u64; 4] {
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

pub(crate) fn handle_mknod(msg: &IpcMessage) -> [u64; 4] {
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
    if n != 0 {
        host_shared_block_mut()[..n]
            .copy_from_slice(&shared_block()[block_offset..block_offset + n]);
        fence(Ordering::SeqCst);
    }
    [XV6_OK, n as u64, inode.typ as u64, inode.size as u64]
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
