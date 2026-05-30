use core::sync::atomic::{Ordering, fence};

use sel4_user::{log, print_u64, read_u16, write_u16};
use xv6_abi::{DIRENT_SIZE, DIRSIZ, FS_BLOCK_SIZE, T_DEVICE, T_DIR, T_FILE, XV6_FS_ROOT_INUM};

use crate::block::{read_disk_block, shared_block, shared_block_mut, write_disk_block};
use crate::inode::{alloc_inode_with, bmap_alloc, data_block, free_inode, read_inode, write_inode};
use crate::types::{Dinode, DirEntryLoc};

pub(crate) fn count_dir_entries(inode: &Dinode) -> usize {
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

pub(crate) fn lookup_root_name(name: &[u8]) -> Option<(u32, Dinode)> {
    let root = read_inode(XV6_FS_ROOT_INUM)?;
    lookup_dir_name(&root, name).and_then(|inum| read_inode(inum).map(|inode| (inum, inode)))
}

fn lookup_dir_name(dir: &Dinode, name: &[u8]) -> Option<u32> {
    find_dir_entry(dir, name).map(|loc| loc.inum)
}

pub(crate) fn find_dir_entry(dir: &Dinode, name: &[u8]) -> Option<DirEntryLoc> {
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

pub(crate) fn add_dir_entry_to_inode(
    dir_inum: u32,
    dir: &mut Dinode,
    name: &[u8],
    target_inum: u32,
) -> bool {
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

pub(crate) fn is_dir_empty(dir: &Dinode) -> bool {
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

pub(crate) fn clear_dirent(loc: DirEntryLoc) -> bool {
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

pub(crate) fn lookup_path_from(cwd_inum: u32, path: &[u8]) -> Option<(u32, Dinode)> {
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

pub(crate) fn lookup_parent_from(
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

pub(crate) fn create_node_from(
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

pub(crate) fn log_bytes(bytes: &[u8]) {
    let mut i = 0usize;
    while i < bytes.len() {
        sel4_user::putchar(bytes[i]);
        i += 1;
    }
}
