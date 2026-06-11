use core::sync::atomic::{Ordering, fence};

use sel4_user::{IpcMessage, msg_info, read_u16, read_u32, sel4_send, warn};
use xv6_abi::{
    DIRENT_SIZE, DIRSIZ, DiskRequestOp, FS_BLOCK_SIZE, MAX_PATH_BYTES, XV6_ABI_VERSION,
    XV6_FS_NDIRECT, XV6_FS_NINDIRECT, XV6_FS_ROOT_INUM, XV6_VFS_REPLY_ENDPOINT_CPTR, Xv6FileType,
    Xv6OpenFlag, Xv6Protocol, Xv6Status,
};

use crate::block::{begin_read_only, handle_transactional_async, handle_transactional_future};
use crate::disk::{
    ScratchSlot, alloc_scratch_slot_async, copy_host_shared_block_from, disk_data_request,
};
use crate::inode::{free_inode, inode_open_refs, release_inode, retain_inode};
use crate::ops::{handle_open, handle_write_async, handle_write_sync};
use crate::types::{DINODE_SIZE, DINODES_PER_BLOCK, Dinode, FS_STATE};

pub(crate) enum RequestResult {
    Reply([u64; 4]),
    Deferred,
}

#[derive(Copy, Clone)]
enum ReplyTarget {
    Caller,
    Vfs { request_id: u64 },
}

pub(crate) async fn handle_fstat(msg: &IpcMessage) -> RequestResult {
    let Some(target) = reply_target(msg) else {
        warn!("xv6fs-server: bad fstat protocol");
        return RequestResult::Reply([Xv6Status::InvalidArgument.raw(), 0, 0, 0]);
    };
    let state = FS_STATE.get();
    if !state.ready {
        warn!("xv6fs-server: fstat before init");
        return immediate_result(target, [Xv6Status::InvalidArgument.raw(), 0, 0, 0]);
    }
    let inum = msg.mrs[2] as u32;
    if inum == 0 || inum >= state.superblock.ninodes {
        return immediate_result(target, [Xv6Status::InvalidArgument.raw(), 0, 0, 0]);
    }

    immediate_result(target, fstat_async(inum).await)
}

pub(crate) async fn handle_read(msg: &IpcMessage, expected_type: u16) -> RequestResult {
    let Some(target) = reply_target(msg) else {
        warn!("xv6fs-server: bad read protocol");
        return RequestResult::Reply([Xv6Status::InvalidArgument.raw(), 0, 0, 0]);
    };
    let state = FS_STATE.get();
    if !state.ready {
        warn!("xv6fs-server: read before init");
        return immediate_result(target, [Xv6Status::InvalidArgument.raw(), 0, 0, 0]);
    }
    let inum = msg.mrs[2] as u32;
    if inum == 0 || inum >= state.superblock.ninodes {
        return immediate_result(target, [Xv6Status::InvalidArgument.raw(), 0, 0, 0]);
    }
    let offset = msg.mrs[3] as usize;
    let max_len = core::cmp::min(msg.mrs[4] as usize, FS_BLOCK_SIZE);

    immediate_result(
        target,
        read_async(inum, offset, max_len, expected_type).await,
    )
}

pub(crate) async fn handle_lookup_dir(msg: &IpcMessage) -> RequestResult {
    let Some(target) = reply_target(msg) else {
        warn!("xv6fs-server: bad lookup-dir protocol");
        return RequestResult::Reply([Xv6Status::InvalidArgument.raw(), 0, 0, 0]);
    };
    let state = FS_STATE.get();
    if !state.ready {
        warn!("xv6fs-server: lookup-dir before init");
        return immediate_result(target, [Xv6Status::InvalidArgument.raw(), 0, 0, 0]);
    }
    let path_len = msg.mrs[4] as usize;
    let mut path = [0u8; MAX_PATH_BYTES];
    if !unpack_path(&msg.mrs, 5, path_len, &mut path) {
        return immediate_result(target, [Xv6Status::InvalidArgument.raw(), 0, 0, 0]);
    }
    let cwd_inum = if path[0] == b'/' {
        XV6_FS_ROOT_INUM
    } else {
        msg.mrs[2] as u32
    };
    if cwd_inum == 0 || cwd_inum >= state.superblock.ninodes {
        return immediate_result(target, [Xv6Status::InvalidArgument.raw(), 0, 0, 0]);
    }

    immediate_result(target, lookup_dir_async(cwd_inum, path, path_len).await)
}

pub(crate) async fn handle_open_at(msg: IpcMessage) -> RequestResult {
    let Some(target) = reply_target(&msg) else {
        warn!("xv6fs-server: bad open protocol");
        return RequestResult::Reply([Xv6Status::InvalidArgument.raw(), 0, 0, 0]);
    };
    let state = FS_STATE.get();
    if !state.ready {
        warn!("xv6fs-server: open before init");
        return immediate_result(target, [Xv6Status::InvalidArgument.raw(), 0, 0, 0]);
    }
    let flags = msg.mrs[3] as u32;
    if flags & (Xv6OpenFlag::Create.raw() | Xv6OpenFlag::Truncate.raw()) != 0 {
        let reply = handle_transactional_async(|| handle_open(&msg)).await;
        return immediate_result(target, reply);
    }

    let path_len = msg.mrs[4] as usize;
    let mut path = [0u8; MAX_PATH_BYTES];
    if !unpack_path(&msg.mrs, 5, path_len, &mut path) {
        return immediate_result(target, [Xv6Status::InvalidArgument.raw(), 0, 0, 0]);
    }
    let cwd_inum = if path[0] == b'/' {
        XV6_FS_ROOT_INUM
    } else {
        msg.mrs[2] as u32
    };
    if cwd_inum == 0 || cwd_inum >= state.superblock.ninodes {
        return immediate_result(target, [Xv6Status::InvalidArgument.raw(), 0, 0, 0]);
    }

    let writable = flags & (Xv6OpenFlag::WriteOnly.raw() | Xv6OpenFlag::ReadWrite.raw()) != 0;
    immediate_result(
        target,
        open_existing_async(cwd_inum, path, path_len, writable).await,
    )
}

pub(crate) async fn handle_retain(msg: &IpcMessage) -> RequestResult {
    let Some(target) = reply_target(msg) else {
        warn!("xv6fs-server: bad retain protocol");
        return RequestResult::Reply([Xv6Status::InvalidArgument.raw(), 0, 0, 0]);
    };
    let state = FS_STATE.get();
    if !state.ready {
        warn!("xv6fs-server: retain before init");
        return immediate_result(target, [Xv6Status::InvalidArgument.raw(), 0, 0, 0]);
    }
    let inum = msg.mrs[2] as u32;
    if inum == 0 || inum >= state.superblock.ninodes {
        return immediate_result(target, [Xv6Status::InvalidArgument.raw(), 0, 0, 0]);
    }

    immediate_result(target, retain_async(inum).await)
}

pub(crate) async fn handle_release(msg: &IpcMessage) -> RequestResult {
    let Some(target) = reply_target(msg) else {
        warn!("xv6fs-server: bad release protocol");
        return RequestResult::Reply([Xv6Status::InvalidArgument.raw(), 0, 0, 0]);
    };
    let state = FS_STATE.get();
    if !state.ready {
        warn!("xv6fs-server: release before init");
        return immediate_result(target, [Xv6Status::InvalidArgument.raw(), 0, 0, 0]);
    }
    let inum = msg.mrs[2] as u32;
    if inum == 0 || inum >= state.superblock.ninodes {
        return immediate_result(target, [Xv6Status::InvalidArgument.raw(), 0, 0, 0]);
    }

    immediate_result(target, release_async(inum).await)
}

pub(crate) async fn handle_transactional_op(
    msg: IpcMessage,
    op: fn(&IpcMessage) -> [u64; 4],
) -> RequestResult {
    let Some(target) = reply_target(&msg) else {
        return RequestResult::Reply([Xv6Status::InvalidArgument.raw(), 0, 0, 0]);
    };
    let reply = handle_transactional_async(|| op(&msg)).await;
    immediate_result(target, reply)
}

pub(crate) async fn handle_write(msg: IpcMessage) -> RequestResult {
    let Some(target) = reply_target(&msg) else {
        return RequestResult::Reply([Xv6Status::InvalidArgument.raw(), 0, 0, 0]);
    };
    let reply = if msg.mrs[4] <= 16 {
        handle_transactional_async(|| handle_write_sync(&msg)).await
    } else {
        handle_transactional_future(handle_write_async(&msg)).await
    };
    immediate_result(target, reply)
}

fn immediate_result(target: ReplyTarget, reply_mrs: [u64; 4]) -> RequestResult {
    match target {
        ReplyTarget::Caller => RequestResult::Reply(reply_mrs),
        ReplyTarget::Vfs { .. } => {
            send_async_reply(target, reply_mrs);
            RequestResult::Deferred
        }
    }
}

fn reply_target(msg: &IpcMessage) -> Option<ReplyTarget> {
    if msg.mrs[0] == Xv6Protocol::VfsToXv6Fs.raw() && msg.mrs[1] == XV6_ABI_VERSION {
        Some(ReplyTarget::Caller)
    } else if msg.mrs[0] == Xv6Protocol::VfsToXv6FsAsync.raw() && msg.mrs[1] != 0 {
        Some(ReplyTarget::Vfs {
            request_id: msg.mrs[1],
        })
    } else {
        None
    }
}

async fn fstat_async(inum: u32) -> [u64; 4] {
    let _read_guard = begin_read_only().await;
    let scratch = alloc_scratch_slot_async().await;
    let Some(inode) = read_inode_async(inum, &scratch).await else {
        return [Xv6Status::InvalidArgument.raw(), 0, 0, 0];
    };
    [
        Xv6Status::Ok.raw(),
        inode.typ as u64,
        inode.nlink as u64,
        inode.size as u64,
    ]
}

async fn retain_async(inum: u32) -> [u64; 4] {
    let _read_guard = begin_read_only().await;
    let scratch = alloc_scratch_slot_async().await;
    let Some(inode) = read_inode_async(inum, &scratch).await else {
        return [Xv6Status::InvalidArgument.raw(), 0, 0, 0];
    };
    if inode.typ == 0 {
        return [Xv6Status::InvalidArgument.raw(), 0, 0, 0];
    }
    if !retain_inode(inum) {
        return [Xv6Status::InvalidArgument.raw(), 0, 0, 0];
    }
    [
        Xv6Status::Ok.raw(),
        inum as u64,
        inode.nlink as u64,
        inode.size as u64,
    ]
}

async fn release_async(inum: u32) -> [u64; 4] {
    let nlink = {
        let _read_guard = begin_read_only().await;
        let scratch = alloc_scratch_slot_async().await;
        let Some(inode) = read_inode_async(inum, &scratch).await else {
            return [Xv6Status::InvalidArgument.raw(), 0, 0, 0];
        };
        if inode.typ == 0 {
            return [Xv6Status::InvalidArgument.raw(), 0, 0, 0];
        }
        inode.nlink
    };
    if !release_inode(inum) {
        return [Xv6Status::InvalidArgument.raw(), 0, 0, 0];
    }
    if nlink != 0 || inode_open_refs(inum) != 0 {
        return [Xv6Status::Ok.raw(), 0, 0, 0];
    }
    handle_transactional_async(|| {
        if free_inode(inum) {
            [Xv6Status::Ok.raw(), 0, 0, 0]
        } else {
            [Xv6Status::InvalidArgument.raw(), 0, 0, 0]
        }
    })
    .await
}

async fn open_existing_async(
    cwd_inum: u32,
    path: [u8; MAX_PATH_BYTES],
    path_len: usize,
    writable: bool,
) -> [u64; 4] {
    let _read_guard = begin_read_only().await;
    let scratch = alloc_scratch_slot_async().await;
    let Some((inum, inode)) = lookup_path_async(cwd_inum, &path, path_len, &scratch).await else {
        return [Xv6Status::InvalidArgument.raw(), 0, 0, 0];
    };
    if inode.typ == 0 || (inode.typ == Xv6FileType::Directory.raw() as i16 && writable) {
        return [Xv6Status::InvalidArgument.raw(), 0, 0, 0];
    }
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

async fn read_async(inum: u32, offset: usize, max_len: usize, expected_type: u16) -> [u64; 4] {
    let _read_guard = begin_read_only().await;
    let scratch = alloc_scratch_slot_async().await;
    let Some(inode) = read_inode_async(inum, &scratch).await else {
        return [Xv6Status::InvalidArgument.raw(), 0, 0, 0];
    };
    if inode.typ != expected_type as i16 {
        return [Xv6Status::InvalidArgument.raw(), 0, 0, 0];
    }
    if max_len == 0 || offset >= inode.size as usize {
        return [Xv6Status::Ok.raw(), 0, inode.typ as u64, inode.size as u64];
    }

    let file_block_index = offset / FS_BLOCK_SIZE;
    let Some(blockno) = data_blockno_async(&inode, file_block_index, &scratch).await else {
        return [Xv6Status::InvalidArgument.raw(), 0, 0, 0];
    };
    let reply = disk_data_request(DiskRequestOp::Read.raw(), blockno, scratch.value()).await;
    if !disk_reply_matches(reply, blockno) {
        return [Xv6Status::InvalidArgument.raw(), 0, 0, 0];
    }

    let size = inode.size as usize;
    let block_offset = offset % FS_BLOCK_SIZE;
    let n = core::cmp::min(
        max_len,
        core::cmp::min(size - offset, FS_BLOCK_SIZE - block_offset),
    );
    fence(Ordering::SeqCst);
    scratch.with_block(|block| {
        copy_host_shared_block_from(&block[block_offset..block_offset + n]);
    });
    fence(Ordering::SeqCst);
    [
        Xv6Status::Ok.raw(),
        n as u64,
        inode.typ as u64,
        inode.size as u64,
    ]
}

async fn lookup_dir_async(cwd_inum: u32, path: [u8; MAX_PATH_BYTES], path_len: usize) -> [u64; 4] {
    let _read_guard = begin_read_only().await;
    let scratch = alloc_scratch_slot_async().await;
    let Some((cur_inum, cur)) = lookup_path_async(cwd_inum, &path, path_len, &scratch).await else {
        return [Xv6Status::InvalidArgument.raw(), 0, 0, 0];
    };
    if cur.typ == Xv6FileType::Directory.raw() as i16 {
        [Xv6Status::Ok.raw(), cur_inum as u64, cur.size as u64, 0]
    } else {
        [Xv6Status::InvalidArgument.raw(), 0, 0, 0]
    }
}

async fn lookup_path_async(
    cwd_inum: u32,
    path: &[u8; MAX_PATH_BYTES],
    path_len: usize,
    scratch: &ScratchSlot,
) -> Option<(u32, Dinode)> {
    let mut cur_inum = cwd_inum;
    let mut cur = read_inode_async(cur_inum, scratch).await?;
    let mut pos = 0usize;
    loop {
        while pos < path_len && path[pos] == b'/' {
            pos += 1;
        }
        if pos >= path_len {
            return Some((cur_inum, cur));
        }

        let start = pos;
        while pos < path_len && path[pos] != b'/' {
            pos += 1;
        }
        let component = &path[start..pos];
        if component == b"." {
            continue;
        }
        if cur.typ != Xv6FileType::Directory.raw() as i16 {
            return None;
        }
        let next_inum = lookup_component_async(&cur, component, scratch).await?;
        cur_inum = next_inum;
        cur = read_inode_async(cur_inum, scratch).await?;
    }
}

async fn lookup_component_async(
    dir: &Dinode,
    component: &[u8],
    scratch: &ScratchSlot,
) -> Option<u32> {
    let name_len = core::cmp::min(component.len(), DIRSIZ);
    if name_len == 0 {
        return None;
    }
    let total = dir.size as usize / DIRENT_SIZE;
    let entries_per_block = FS_BLOCK_SIZE / DIRENT_SIZE;
    let mut dir_entry_index = 0usize;
    while dir_entry_index < total {
        let file_block_index = dir_entry_index / entries_per_block;
        let block_start = file_block_index * entries_per_block;
        let entries = core::cmp::min(total - block_start, entries_per_block);
        let blockno = data_blockno_async(dir, file_block_index, scratch).await?;
        let reply = disk_data_request(DiskRequestOp::Read.raw(), blockno, scratch.value()).await;
        if !disk_reply_matches(reply, blockno) {
            return None;
        }
        let found = scratch.with_block(|block| {
            let mut entry = dir_entry_index - block_start;
            while entry < entries {
                let off = entry * DIRENT_SIZE;
                let inum = read_u16(block, off) as u32;
                let stored_len = dirent_name_len(block, off + 2);
                if inum != 0
                    && stored_len == name_len
                    && dirent_name_eq(block, off + 2, component, name_len)
                {
                    return Some(inum);
                }
                entry += 1;
            }
            None
        });
        if let Some(inum) = found {
            return Some(inum);
        }
        dir_entry_index = block_start + entries;
    }
    None
}

async fn read_inode_async(inum: u32, scratch: &ScratchSlot) -> Option<Dinode> {
    let state = FS_STATE.get();
    if !state.ready || inum == 0 || inum >= state.superblock.ninodes {
        return None;
    }
    let blockno = inode_blockno(inum);
    let reply = disk_data_request(DiskRequestOp::Read.raw(), blockno, scratch.value()).await;
    if !disk_reply_matches(reply, blockno) {
        return None;
    }
    Some(scratch.with_block(|block| read_inode_from_block(inum, block)))
}

async fn data_blockno_async(
    inode: &Dinode,
    file_block_index: usize,
    scratch: &ScratchSlot,
) -> Option<u32> {
    if file_block_index < XV6_FS_NDIRECT {
        let blockno = inode.addrs[file_block_index];
        return (blockno != 0).then_some(blockno);
    }

    let indirect_index = file_block_index - XV6_FS_NDIRECT;
    if indirect_index >= XV6_FS_NINDIRECT || inode.addrs[XV6_FS_NDIRECT] == 0 {
        return None;
    }
    let indirect = inode.addrs[XV6_FS_NDIRECT];
    let reply = disk_data_request(DiskRequestOp::Read.raw(), indirect, scratch.value()).await;
    if !disk_reply_matches(reply, indirect) {
        return None;
    }
    let blockno = scratch.with_block(|block| read_u32(block, indirect_index * 4));
    (blockno != 0).then_some(blockno)
}

fn read_inode_from_block(inum: u32, block: &[u8]) -> Dinode {
    let offset = ((inum % DINODES_PER_BLOCK) as usize) * DINODE_SIZE;
    let mut addrs = [0u32; XV6_FS_NDIRECT + 1];
    let mut i = 0usize;
    while i < addrs.len() {
        addrs[i] = read_u32(block, offset + 12 + i * 4);
        i += 1;
    }
    Dinode {
        typ: read_u16(block, offset) as i16,
        major: read_u16(block, offset + 2),
        minor: read_u16(block, offset + 4),
        nlink: read_u16(block, offset + 6),
        size: read_u32(block, offset + 8),
        addrs,
    }
}

fn inode_blockno(inum: u32) -> u32 {
    let state = FS_STATE.get();
    inum / DINODES_PER_BLOCK + state.superblock.inodestart
}

fn disk_reply_matches(reply: [u64; 4], blockno: u32) -> bool {
    reply[0] == Xv6Status::Ok.raw()
        && reply[1] == FS_BLOCK_SIZE as u64
        && reply[2] == blockno as u64
}

fn dirent_name_len(block: &[u8], name_offset: usize) -> usize {
    let mut name_len = 0usize;
    while name_len < DIRSIZ && block[name_offset + name_len] != 0 {
        name_len += 1;
    }
    name_len
}

fn dirent_name_eq(block: &[u8], name_offset: usize, name: &[u8], name_len: usize) -> bool {
    let mut i = 0usize;
    while i < name_len {
        if block[name_offset + i] != name[i] {
            return false;
        }
        i += 1;
    }
    true
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

fn send_async_reply(target: ReplyTarget, reply_mrs: [u64; 4]) {
    if let ReplyTarget::Vfs { request_id } = target {
        unsafe {
            sel4_send(
                XV6_VFS_REPLY_ENDPOINT_CPTR,
                msg_info(Xv6Protocol::VfsToXv6FsAsync.raw(), 0, 0, 5),
                &[
                    request_id,
                    reply_mrs[0],
                    reply_mrs[1],
                    reply_mrs[2],
                    reply_mrs[3],
                ],
            );
        }
    }
}
