#![no_std]
#![no_main]
#![deny(unsafe_attr_outside_unsafe)]
#![deny(unsafe_op_in_unsafe_fn)]

use core::panic::PanicInfo;
use core::sync::atomic::{Ordering, fence};

use sel4_user::{
    IpcMessage, halt_loop, init_ipc_buffer, log, msg_info, msg_label, print_u64, read_u16,
    read_u32, sel4_call, sel4_recv, sel4_reply_recv,
};
use xv6_abi::{
    DIRENT_SIZE, DIRSIZ, DISK_OP_GET_INFO, DISK_OP_READ, FS_BLOCK_SIZE, FS_OP_INIT, FS_OP_OPEN,
    O_CREATE, O_RDWR, O_TRUNC, O_WRONLY, T_DIR, VIRTIO_BLK_SECTOR_SIZE, XV6_ABI_VERSION,
    XV6_DISK_ENDPOINT_CPTR, XV6_DISK_SHARED_BUFFER_VADDR, XV6_EINVAL, XV6_ENOSYS, XV6_FS_MAGIC,
    XV6_FS_NDIRECT, XV6_FS_ROOT_INUM, XV6_FS_TO_DISK_PROTOCOL, XV6_HOST_TO_FS_PROTOCOL, XV6_OK,
    XV6_SERVICE_ENDPOINT_CPTR, Xv6Superblock,
};

const DINODE_SIZE: usize = 64;
const DINODES_PER_BLOCK: u32 = (FS_BLOCK_SIZE / DINODE_SIZE) as u32;

#[derive(Copy, Clone)]
struct FsState {
    ready: bool,
    superblock: Xv6Superblock,
}

impl FsState {
    const fn empty() -> Self {
        Self {
            ready: false,
            superblock: Xv6Superblock {
                magic: 0,
                size: 0,
                nblocks: 0,
                ninodes: 0,
                nlog: 0,
                logstart: 0,
                inodestart: 0,
                bmapstart: 0,
            },
        }
    }
}

#[derive(Copy, Clone)]
struct Dinode {
    typ: i16,
    nlink: u16,
    size: u32,
    addrs: [u32; XV6_FS_NDIRECT + 1],
}

static mut FS_STATE: FsState = FsState::empty();

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
        FS_OP_OPEN => handle_open(msg),
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

    unsafe {
        FS_STATE = FsState {
            ready: true,
            superblock,
        };
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
    if (msg.mrs[3] as u32) & (O_WRONLY | O_RDWR | O_CREATE | O_TRUNC) != 0 {
        log("xv6-fs-server: read-only open rejected\n");
        return [XV6_EINVAL, 0, 0, 0];
    }
    let path_len = msg.mrs[4] as usize;
    let Some((inum, inode)) = lookup_packed_path(&msg.mrs, path_len) else {
        return [XV6_EINVAL, 0, 0, 0];
    };
    [XV6_OK, inum as u64, inode.typ as u64, inode.size as u64]
}

fn read_disk_block(blockno: u32) -> bool {
    let reply = unsafe {
        sel4_call(
            XV6_DISK_ENDPOINT_CPTR,
            msg_info(DISK_OP_READ, 0, 0, 3),
            &[XV6_FS_TO_DISK_PROTOCOL, XV6_ABI_VERSION, blockno as u64],
        )
    };
    if msg_label(reply.info) != 0 || reply.mrs[0] != XV6_OK {
        log("xv6-fs-server: disk read failed block=");
        print_u64(blockno as u64);
        log(" status=");
        print_u64(reply.mrs[0]);
        log("\n");
        return false;
    }
    reply.mrs[1] == FS_BLOCK_SIZE as u64 && reply.mrs[2] == blockno as u64
}

fn read_superblock_from_shared() -> Xv6Superblock {
    let block = shared_block();
    Xv6Superblock {
        magic: read_u32(block, 0),
        size: read_u32(block, 4),
        nblocks: read_u32(block, 8),
        ninodes: read_u32(block, 12),
        nlog: read_u32(block, 16),
        logstart: read_u32(block, 20),
        inodestart: read_u32(block, 24),
        bmapstart: read_u32(block, 28),
    }
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
        nlink: read_u16(block, offset + 6),
        size: read_u32(block, offset + 8),
        addrs,
    })
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
                return Some(inum as u32);
            }
            entry += 1;
        }
        block_index += 1;
    }
    None
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
    None
}

fn lookup_packed_path(mrs: &[u64; 64], path_len: usize) -> Option<(u32, Dinode)> {
    if path_len == 0 || path_len > 128 {
        return None;
    }
    let mut path = [0u8; 128];
    let mut i = 0usize;
    while i < path_len {
        let word = mrs[5 + i / 8];
        path[i] = ((word >> ((i % 8) * 8)) & 0xff) as u8;
        i += 1;
    }
    lookup_path(&path[..path_len])
}

fn lookup_path(path: &[u8]) -> Option<(u32, Dinode)> {
    if path == b"/" || path == b"." {
        return read_inode(XV6_FS_ROOT_INUM).map(|inode| (XV6_FS_ROOT_INUM, inode));
    }
    let mut start = 0usize;
    while start < path.len() && path[start] == b'/' {
        start += 1;
    }
    let mut end = path.len();
    while end > start && path[end - 1] == b'/' {
        end -= 1;
    }
    if start == end {
        return read_inode(XV6_FS_ROOT_INUM).map(|inode| (XV6_FS_ROOT_INUM, inode));
    }
    let component = &path[start..end];
    let mut i = 0usize;
    while i < component.len() {
        if component[i] == b'/' {
            return None;
        }
        i += 1;
    }
    lookup_root_name(component)
}

fn shared_block() -> &'static [u8] {
    unsafe { core::slice::from_raw_parts(XV6_DISK_SHARED_BUFFER_VADDR as *const u8, FS_BLOCK_SIZE) }
}

#[panic_handler]
fn panic(_: &PanicInfo) -> ! {
    log("xv6-fs-server: panic\n");
    halt_loop()
}
