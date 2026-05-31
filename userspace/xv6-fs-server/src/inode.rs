use core::ptr;
use core::sync::atomic::{Ordering, fence};

use sel4_user::{read_u16, read_u32, write_u16, write_u32};
use xv6_abi::{FS_BLOCK_SIZE, XV6_FS_NDIRECT, XV6_FS_NINDIRECT};

use crate::block::{read_disk_block, shared_block, shared_block_mut, write_disk_block};
use crate::types::{
    BPB, DINODE_SIZE, DINODES_PER_BLOCK, Dinode, FS_MAX_TRACKED_INODES, FS_STATE, OPEN_REFS,
};

pub(crate) fn read_inode(inum: u32) -> Option<Dinode> {
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

pub(crate) fn write_inode(inum: u32, inode: &Dinode) -> bool {
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

pub(crate) fn write_inode_meta(inum: u32, inode: &Dinode) -> bool {
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

pub(crate) fn retain_inode(inum: u32) {
    if let Some(index) = inode_ref_index(inum) {
        unsafe {
            OPEN_REFS[index] = OPEN_REFS[index].saturating_add(1);
        }
    }
}

pub(crate) fn release_inode(inum: u32) {
    if let Some(index) = inode_ref_index(inum) {
        unsafe {
            if OPEN_REFS[index] != 0 {
                OPEN_REFS[index] -= 1;
            }
        }
    }
}

pub(crate) fn inode_open_refs(inum: u32) -> u16 {
    if let Some(index) = inode_ref_index(inum) {
        unsafe { OPEN_REFS[index] }
    } else {
        0
    }
}

pub(crate) fn alloc_inode_with(typ: u16, major: u16, minor: u16) -> Option<u32> {
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

pub(crate) fn free_inode(inum: u32) -> bool {
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

pub(crate) fn truncate_inode(inode: &mut Dinode) -> bool {
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
            ptr::copy_nonoverlapping(
                shared_block().as_ptr(),
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

pub(crate) fn write_inode_data(
    inum: u32,
    inode: &mut Dinode,
    offset: usize,
    src: &[u8],
) -> Option<usize> {
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

pub(crate) fn bmap_alloc(inode: &mut Dinode, file_block_index: usize) -> Option<u32> {
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

pub(crate) fn data_block(inode: &Dinode, file_block_index: usize) -> Option<u32> {
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
