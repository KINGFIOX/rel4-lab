use sel4_user::{IpcMessage, LogBytes, info, warn};
use xv6_abi::{
    MAX_PATH_BYTES, XV6_ABI_VERSION, XV6_MAX_FILE_WRITE, Xv6FileType, Xv6OpenFlag, Xv6Protocol,
    Xv6Status,
};

use crate::block::host_shared_write_buffer;
use crate::dir::{
    add_dir_entry_to_inode, clear_dirent, create_node_from, find_dir_entry, is_dir_empty,
    lookup_parent_from, lookup_path_from,
};
use crate::inode::{
    free_inode, inode_open_refs, read_inode, read_inode_async, retain_inode, truncate_inode,
    write_inode, write_inode_data, write_inode_data_async, write_inode_meta,
};
use crate::types::FS_STATE;

pub(crate) fn handle_open(msg: &IpcMessage) -> [u64; 4] {
    if !valid_protocol(msg) {
        warn!("xv6fs-server: bad open protocol");
        return [Xv6Status::InvalidArgument.raw(), 0, 0, 0];
    }
    if !FS_STATE.ready() {
        warn!("xv6fs-server: open before init");
        return [Xv6Status::InvalidArgument.raw(), 0, 0, 0];
    }
    let cwd_inum = msg.mrs[2] as u32;
    let flags = msg.mrs[3] as u32;
    let path_len = msg.mrs[4] as usize;
    let mut path = [0u8; MAX_PATH_BYTES];
    if !unpack_path(&msg.mrs, 5, path_len, &mut path) {
        return [Xv6Status::InvalidArgument.raw(), 0, 0, 0];
    };
    let path = &path[..path_len];
    let writable = flags & (Xv6OpenFlag::WriteOnly.raw() | Xv6OpenFlag::ReadWrite.raw()) != 0;

    if let Some((inum, mut inode)) = lookup_path_from(cwd_inum, path) {
        if inode.typ == Xv6FileType::Directory.raw() as i16 && writable {
            return [Xv6Status::InvalidArgument.raw(), 0, 0, 0];
        }
        if flags & Xv6OpenFlag::Truncate.raw() != 0
            && inode.typ == Xv6FileType::File.raw() as i16
            && writable
        {
            if !truncate_inode(&mut inode) || !write_inode(inum, &inode) {
                return [Xv6Status::InvalidArgument.raw(), 0, 0, 0];
            }
        }
        if !retain_inode(inum) {
            return [Xv6Status::InvalidArgument.raw(), 0, 0, 0];
        }
        return [
            Xv6Status::Ok.raw(),
            inum as u64,
            inode.typ as u64,
            inode.size as u64,
        ];
    }

    if flags & Xv6OpenFlag::Create.raw() == 0 {
        return [Xv6Status::InvalidArgument.raw(), 0, 0, 0];
    }
    let Some((inum, inode)) = create_node_from(cwd_inum, path, Xv6FileType::File.raw(), 0, 0)
    else {
        return [Xv6Status::InvalidArgument.raw(), 0, 0, 0];
    };
    if !retain_inode(inum) {
        return [Xv6Status::InvalidArgument.raw(), 0, 0, 0];
    }
    [
        Xv6Status::Ok.raw(),
        inum as u64,
        inode.typ as u64,
        inode.size as u64,
    ]
}

pub(crate) async fn handle_write_async(msg: &IpcMessage) -> [u64; 4] {
    if !valid_protocol(msg) {
        warn!("xv6fs-server: bad write protocol");
        return [Xv6Status::InvalidArgument.raw(), 0, 0, 0];
    }
    if !FS_STATE.ready() {
        warn!("xv6fs-server: write before init");
        return [Xv6Status::InvalidArgument.raw(), 0, 0, 0];
    }
    let inum = msg.mrs[2] as u32;
    let offset = msg.mrs[3] as usize;
    let len = core::cmp::min(msg.mrs[4] as usize, XV6_MAX_FILE_WRITE);
    let mut data = [0u8; XV6_MAX_FILE_WRITE];
    data[..len].copy_from_slice(&host_shared_write_buffer()[..len]);
    let Some(mut inode) = read_inode_async(inum).await else {
        return [Xv6Status::InvalidArgument.raw(), 0, 0, 0];
    };
    if inode.typ != Xv6FileType::File.raw() as i16 || offset > inode.size as usize {
        return [Xv6Status::InvalidArgument.raw(), 0, 0, 0];
    }
    let Some(written) = write_inode_data_async(inum, &mut inode, offset, &data[..len]).await else {
        return [Xv6Status::InvalidArgument.raw(), 0, 0, 0];
    };
    [
        Xv6Status::Ok.raw(),
        written as u64,
        inode.typ as u64,
        inode.size as u64,
    ]
}

pub(crate) fn handle_write_sync(msg: &IpcMessage) -> [u64; 4] {
    if !valid_protocol(msg) {
        warn!("xv6fs-server: bad write protocol");
        return [Xv6Status::InvalidArgument.raw(), 0, 0, 0];
    }
    if !FS_STATE.ready() {
        warn!("xv6fs-server: write before init");
        return [Xv6Status::InvalidArgument.raw(), 0, 0, 0];
    }
    let inum = msg.mrs[2] as u32;
    let offset = msg.mrs[3] as usize;
    let len = core::cmp::min(msg.mrs[4] as usize, XV6_MAX_FILE_WRITE);
    let src = &host_shared_write_buffer()[..len];
    let Some(mut inode) = read_inode(inum) else {
        return [Xv6Status::InvalidArgument.raw(), 0, 0, 0];
    };
    if inode.typ != Xv6FileType::File.raw() as i16 || offset > inode.size as usize {
        return [Xv6Status::InvalidArgument.raw(), 0, 0, 0];
    }
    let Some(written) = write_inode_data(inum, &mut inode, offset, src) else {
        return [Xv6Status::InvalidArgument.raw(), 0, 0, 0];
    };
    [
        Xv6Status::Ok.raw(),
        written as u64,
        inode.typ as u64,
        inode.size as u64,
    ]
}

pub(crate) fn handle_unlink(msg: &IpcMessage) -> [u64; 4] {
    if !valid_protocol(msg) {
        warn!("xv6fs-server: bad unlink protocol");
        return [Xv6Status::InvalidArgument.raw(), 0, 0, 0];
    }
    if !FS_STATE.ready() {
        warn!("xv6fs-server: unlink before init");
        return [Xv6Status::InvalidArgument.raw(), 0, 0, 0];
    }
    let cwd_inum = msg.mrs[2] as u32;
    let path_len = msg.mrs[3] as usize;
    let mut path = [0u8; MAX_PATH_BYTES];
    if !unpack_path(&msg.mrs, 4, path_len, &mut path) {
        return [Xv6Status::InvalidArgument.raw(), 0, 0, 0];
    }
    let Some((parent_inum, mut parent, name, name_len)) =
        lookup_parent_from(cwd_inum, &path[..path_len], true)
    else {
        return [Xv6Status::InvalidArgument.raw(), 0, 0, 0];
    };
    let name = &name[..name_len];
    let Some(loc) = find_dir_entry(&parent, name) else {
        return [Xv6Status::InvalidArgument.raw(), 0, 0, 0];
    };
    let Some(mut inode) = read_inode(loc.inum) else {
        return [Xv6Status::InvalidArgument.raw(), 0, 0, 0];
    };
    if inode.typ == Xv6FileType::Directory.raw() as i16 && !is_dir_empty(&inode) {
        return [Xv6Status::InvalidArgument.raw(), 0, 0, 0];
    }
    if inode.nlink == 0 {
        return [Xv6Status::InvalidArgument.raw(), 0, 0, 0];
    }
    if !clear_dirent(loc) {
        return [Xv6Status::InvalidArgument.raw(), 0, 0, 0];
    }
    if inode.typ == Xv6FileType::Directory.raw() as i16 {
        if parent.nlink == 0 {
            return [Xv6Status::InvalidArgument.raw(), 0, 0, 0];
        }
        parent.nlink -= 1;
        if !write_inode(parent_inum, &parent) {
            return [Xv6Status::InvalidArgument.raw(), 0, 0, 0];
        }
    }
    inode.nlink -= 1;
    if !write_inode_meta(loc.inum, &inode) {
        return [Xv6Status::InvalidArgument.raw(), 0, 0, 0];
    }
    if inode.nlink == 0 && inode_open_refs(loc.inum) == 0 {
        let _ = free_inode(loc.inum);
    }
    info!(
        "xv6fs-server: unlink {} ino={} nlink={} size={}",
        LogBytes(name),
        loc.inum,
        inode.nlink,
        inode.size
    );
    [Xv6Status::Ok.raw(), loc.inum as u64, inode.nlink as u64, 0]
}

pub(crate) fn handle_link(msg: &IpcMessage) -> [u64; 4] {
    if !valid_protocol(msg) {
        warn!("xv6fs-server: bad link protocol");
        return [Xv6Status::InvalidArgument.raw(), 0, 0, 0];
    }
    if !FS_STATE.ready() {
        warn!("xv6fs-server: link before init");
        return [Xv6Status::InvalidArgument.raw(), 0, 0, 0];
    }
    let cwd_inum = msg.mrs[2] as u32;
    let old_len = msg.mrs[3] as usize;
    let new_len = msg.mrs[4] as usize;
    let old_words = old_len.div_ceil(8);
    let mut old_path = [0u8; MAX_PATH_BYTES];
    let mut new_path = [0u8; MAX_PATH_BYTES];
    if !unpack_path(&msg.mrs, 5, old_len, &mut old_path)
        || !unpack_path(&msg.mrs, 5 + old_words, new_len, &mut new_path)
    {
        return [Xv6Status::InvalidArgument.raw(), 0, 0, 0];
    }
    let Some((old_inum, mut inode)) = lookup_path_from(cwd_inum, &old_path[..old_len]) else {
        return [Xv6Status::InvalidArgument.raw(), 0, 0, 0];
    };
    if inode.typ != Xv6FileType::File.raw() as i16 || inode.nlink == u16::MAX {
        return [Xv6Status::InvalidArgument.raw(), 0, 0, 0];
    }
    let Some((parent_inum, mut parent, new_name, new_name_len)) =
        lookup_parent_from(cwd_inum, &new_path[..new_len], true)
    else {
        return [Xv6Status::InvalidArgument.raw(), 0, 0, 0];
    };
    let new_name = &new_name[..new_name_len];
    if find_dir_entry(&parent, new_name).is_some() {
        return [Xv6Status::InvalidArgument.raw(), 0, 0, 0];
    }

    inode.nlink += 1;
    if !write_inode(old_inum, &inode) {
        return [Xv6Status::InvalidArgument.raw(), 0, 0, 0];
    }
    if !add_dir_entry_to_inode(parent_inum, &mut parent, new_name, old_inum) {
        inode.nlink -= 1;
        let _ = write_inode(old_inum, &inode);
        return [Xv6Status::InvalidArgument.raw(), 0, 0, 0];
    }
    info!(
        "xv6fs-server: link {} -> {} ino={} nlink={}",
        LogBytes(&old_path[..old_len]),
        LogBytes(new_name),
        old_inum,
        inode.nlink
    );
    [Xv6Status::Ok.raw(), old_inum as u64, inode.nlink as u64, 0]
}

pub(crate) fn handle_mkdir(msg: &IpcMessage) -> [u64; 4] {
    if !valid_protocol(msg) {
        warn!("xv6fs-server: bad mkdir protocol");
        return [Xv6Status::InvalidArgument.raw(), 0, 0, 0];
    }
    if !FS_STATE.ready() {
        warn!("xv6fs-server: mkdir before init");
        return [Xv6Status::InvalidArgument.raw(), 0, 0, 0];
    }
    let cwd_inum = msg.mrs[2] as u32;
    let path_len = msg.mrs[3] as usize;
    let mut path = [0u8; MAX_PATH_BYTES];
    if !unpack_path(&msg.mrs, 4, path_len, &mut path) {
        return [Xv6Status::InvalidArgument.raw(), 0, 0, 0];
    }
    let Some((inum, inode)) = create_node_from(
        cwd_inum,
        &path[..path_len],
        Xv6FileType::Directory.raw(),
        0,
        0,
    ) else {
        return [Xv6Status::InvalidArgument.raw(), 0, 0, 0];
    };
    [
        Xv6Status::Ok.raw(),
        inum as u64,
        inode.nlink as u64,
        inode.size as u64,
    ]
}

pub(crate) fn handle_mknod(msg: &IpcMessage) -> [u64; 4] {
    if !valid_protocol(msg) {
        warn!("xv6fs-server: bad mknod protocol");
        return [Xv6Status::InvalidArgument.raw(), 0, 0, 0];
    }
    if !FS_STATE.ready() {
        warn!("xv6fs-server: mknod before init");
        return [Xv6Status::InvalidArgument.raw(), 0, 0, 0];
    }
    let cwd_inum = msg.mrs[2] as u32;
    let major = msg.mrs[3] as u16;
    let minor = msg.mrs[4] as u16;
    let path_len = msg.mrs[5] as usize;
    let mut path = [0u8; MAX_PATH_BYTES];
    if !unpack_path(&msg.mrs, 6, path_len, &mut path) {
        return [Xv6Status::InvalidArgument.raw(), 0, 0, 0];
    }
    let Some((inum, inode)) = create_node_from(
        cwd_inum,
        &path[..path_len],
        Xv6FileType::Device.raw(),
        major,
        minor,
    ) else {
        return [Xv6Status::InvalidArgument.raw(), 0, 0, 0];
    };
    [
        Xv6Status::Ok.raw(),
        inum as u64,
        inode.major as u64,
        inode.minor as u64,
    ]
}

fn valid_protocol(msg: &IpcMessage) -> bool {
    (msg.mrs[0] == Xv6Protocol::VfsToXv6Fs.raw() && msg.mrs[1] == XV6_ABI_VERSION)
        || (msg.mrs[0] == Xv6Protocol::VfsToXv6FsAsync.raw() && msg.mrs[1] != 0)
}

fn unpack_path(
    mrs: &[u64; 64],
    start: usize,
    path_len: usize,
    out: &mut [u8; MAX_PATH_BYTES],
) -> bool {
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
