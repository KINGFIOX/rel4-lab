use core::cmp::min;

use linux_abi::{
    AT_REMOVEDIR, CONSOLE_INO, EEXIST, EINVAL, EISDIR, ENOENT, ENOMEM, ENOSPC, ENOTDIR, ENOTEMPTY,
    FileKind, MAX_PATH_BYTES, O_CREAT, O_DIRECTORY, O_EXCL, O_TRUNC, ROOT_INO, S_IFCHR, S_IFDIR,
    S_IFREG, SEEK_CUR, SEEK_END, SEEK_SET,
};

pub(crate) const MAX_INODES: usize = 256;
pub(crate) const MAX_DIR_ENTRIES: usize = 1024;
const NAME_MAX: usize = 64;
const MAX_DATA_PAGES: usize = 128;
const PAGE: usize = 4096;
const MAX_FILE_PAGES: usize = 256;

#[derive(Copy, Clone)]
pub(crate) struct Inode {
    pub(crate) used: bool,
    pub(crate) kind: FileKind,
    pub(crate) nlink: u16,
    pub(crate) refs: u16,
    pub(crate) _mode: u32,
    pub(crate) size: usize,
    pub(crate) parent: u32,
    src: InodeData,
}

#[derive(Copy, Clone)]
enum InodeData {
    None,
    Embedded {
        offset: usize,
        len: usize,
    },
    Pages {
        pages: [u16; MAX_FILE_PAGES],
        n: u16,
    },
}

impl Inode {
    const fn empty() -> Self {
        Self {
            used: false,
            kind: FileKind::File,
            nlink: 0,
            refs: 0,
            _mode: 0,
            size: 0,
            parent: 0,
            src: InodeData::None,
        }
    }
}

#[derive(Copy, Clone)]
struct Dirent {
    used: bool,
    parent: u32,
    child: u32,
    name: [u8; NAME_MAX],
    name_len: u8,
}

impl Dirent {
    const fn empty() -> Self {
        Self {
            used: false,
            parent: 0,
            child: 0,
            name: [0; NAME_MAX],
            name_len: 0,
        }
    }
}

struct Ramfs {
    inodes: [Inode; MAX_INODES],
    dirents: [Dirent; MAX_DIR_ENTRIES],
    pages: [[u8; PAGE]; MAX_DATA_PAGES],
    page_used: [bool; MAX_DATA_PAGES],
}

impl Ramfs {
    const fn new() -> Self {
        Self {
            inodes: [Inode::empty(); MAX_INODES],
            dirents: [Dirent::empty(); MAX_DIR_ENTRIES],
            pages: [[0; PAGE]; MAX_DATA_PAGES],
            page_used: [false; MAX_DATA_PAGES],
        }
    }
}

use core::cell::UnsafeCell;

struct RamfsCell {
    inner: UnsafeCell<Ramfs>,
}

unsafe impl Sync for RamfsCell {}

static RAMFS: RamfsCell = RamfsCell {
    inner: UnsafeCell::new(Ramfs::new()),
};

fn fs() -> &'static mut Ramfs {
    unsafe { &mut *RAMFS.inner.get() }
}

pub(crate) fn reset() {
    let fs = fs();
    let mut i = 0usize;
    while i < MAX_INODES {
        fs.inodes[i] = Inode::empty();
        i += 1;
    }
    i = 0;
    while i < MAX_DIR_ENTRIES {
        fs.dirents[i] = Dirent::empty();
        i += 1;
    }
    i = 0;
    while i < MAX_DATA_PAGES {
        fs.page_used[i] = false;
        i += 1;
    }
}

pub(crate) fn init_root() -> bool {
    let fs = fs();
    if ROOT_INO as usize >= MAX_INODES {
        return false;
    }
    fs.inodes[ROOT_INO as usize] = Inode {
        used: true,
        kind: FileKind::Directory,
        nlink: 2,
        refs: 1,
        _mode: S_IFDIR | 0o755,
        size: 0,
        parent: ROOT_INO,
        src: InodeData::None,
    };
    true
}

pub(crate) fn inode(inum: u32) -> Option<Inode> {
    let idx = inum as usize;
    if idx < MAX_INODES && fs().inodes[idx].used {
        Some(fs().inodes[idx])
    } else {
        None
    }
}

pub(crate) fn retain(inum: u32) -> bool {
    let Some(node) = fs().inodes.get_mut(inum as usize) else {
        return false;
    };
    if !node.used || node.refs == u16::MAX {
        return false;
    }
    node.refs += 1;
    true
}

pub(crate) fn release(inum: u32) -> bool {
    let Some(node) = fs().inodes.get_mut(inum as usize) else {
        return false;
    };
    if !node.used {
        return false;
    }
    if node.refs > 0 {
        node.refs -= 1;
    }
    true
}

pub(crate) fn walk(path: &[u8]) -> Result<u32, i32> {
    if path.is_empty() {
        return Err(ENOENT);
    }
    let mut cur = ROOT_INO;
    let mut pos = 0usize;
    while pos < path.len() {
        while pos < path.len() && path[pos] == b'/' {
            pos += 1;
        }
        if pos >= path.len() {
            break;
        }
        let start = pos;
        while pos < path.len() && path[pos] != b'/' {
            pos += 1;
        }
        let name = &path[start..pos];
        if name == b"." {
            continue;
        }
        if name == b".." {
            if let Some(node) = inode(cur) {
                cur = node.parent;
            }
            continue;
        }
        cur = lookup(cur, name).ok_or(ENOENT)?;
    }
    Ok(cur)
}

pub(crate) fn lookup(dir: u32, name: &[u8]) -> Option<u32> {
    let fs = fs();
    let mut i = 0usize;
    while i < MAX_DIR_ENTRIES {
        let dent = fs.dirents[i];
        if dent.used && dent.parent == dir && name_eq(&dent, name) {
            return Some(dent.child);
        }
        i += 1;
    }
    None
}

fn name_eq(dent: &Dirent, name: &[u8]) -> bool {
    dent.name_len as usize == name.len() && dent.name[..name.len()] == *name
}

fn split_parent<'a>(path: &'a [u8]) -> Option<(u32, &'a [u8])> {
    if path.is_empty() {
        return None;
    }
    let mut end = path.len();
    while end > 0 && path[end - 1] == b'/' {
        end -= 1;
    }
    if end == 0 {
        return Some((ROOT_INO, b""));
    }
    let mut start = end;
    while start > 0 && path[start - 1] != b'/' {
        start -= 1;
    }
    let name = &path[start..end];
    let parent_path = if start <= 1 { b"/" } else { &path[..start - 1] };
    let parent = walk(parent_path).ok()?;
    Some((parent, name))
}

pub(crate) fn open_path(path: &[u8], flags: u32) -> Result<(u32, FileKind, usize), i32> {
    match walk(path) {
        Ok(inum) => {
            if flags & O_EXCL != 0 && flags & O_CREAT != 0 {
                return Err(EEXIST);
            }
            let node = inode(inum).ok_or(ENOENT)?;
            if flags & O_DIRECTORY != 0 && node.kind != FileKind::Directory {
                return Err(ENOTDIR);
            }
            if node.kind == FileKind::Directory && (flags & O_TRUNC != 0 || wants_file_write(flags))
            {
                return Err(EISDIR);
            }
            if flags & O_TRUNC != 0 && node.kind == FileKind::File {
                truncate(inum)?;
            }
            retain(inum);
            let node = inode(inum).ok_or(ENOENT)?;
            Ok((inum, node.kind, node.size))
        }
        Err(ENOENT) if flags & O_CREAT != 0 => {
            let (parent, name) = split_parent(path).ok_or(ENOENT)?;
            if name.is_empty() || name == b"." || name == b".." {
                return Err(EINVAL);
            }
            let inum = create_file(parent, name, FileKind::File, S_IFREG | 0o644)?;
            retain(inum);
            Ok((inum, FileKind::File, 0))
        }
        Err(e) => Err(e),
    }
}

fn wants_file_write(flags: u32) -> bool {
    linux_abi::open_writable(flags)
}

pub(crate) fn mkdir(path: &[u8]) -> Result<u32, i32> {
    if walk(path).is_ok() {
        return Err(EEXIST);
    }
    let (parent, name) = split_parent(path).ok_or(ENOENT)?;
    if name.is_empty() || name == b"." || name == b".." {
        return Err(EINVAL);
    }
    create_file(parent, name, FileKind::Directory, S_IFDIR | 0o755)
}

pub(crate) fn unlink(path: &[u8], flags: u32) -> Result<(), i32> {
    let inum = walk(path).map_err(|_| ENOENT)?;
    let node = inode(inum).ok_or(ENOENT)?;
    if flags & AT_REMOVEDIR != 0 {
        if node.kind != FileKind::Directory {
            return Err(ENOTDIR);
        }
        if !dir_empty(inum) {
            return Err(ENOTEMPTY);
        }
    } else if node.kind == FileKind::Directory {
        return Err(EISDIR);
    }
    let (parent, name) = split_parent(path).ok_or(ENOENT)?;
    remove_dirent(parent, name)?;
    let fs = fs();
    let node = &mut fs.inodes[inum as usize];
    if node.nlink > 0 {
        node.nlink -= 1;
    }
    if node.nlink == 0 && node.refs == 0 && node.kind != FileKind::Directory {
        free_inode_data(inum);
        fs.inodes[inum as usize] = Inode::empty();
    }
    Ok(())
}

fn dir_empty(dir: u32) -> bool {
    let fs = fs();
    let mut i = 0usize;
    while i < MAX_DIR_ENTRIES {
        if fs.dirents[i].used && fs.dirents[i].parent == dir {
            return false;
        }
        i += 1;
    }
    true
}

fn remove_dirent(parent: u32, name: &[u8]) -> Result<(), i32> {
    let fs = fs();
    let mut i = 0usize;
    while i < MAX_DIR_ENTRIES {
        if fs.dirents[i].used && fs.dirents[i].parent == parent && name_eq(&fs.dirents[i], name) {
            fs.dirents[i] = Dirent::empty();
            return Ok(());
        }
        i += 1;
    }
    Err(ENOENT)
}

pub(crate) fn create_file(parent: u32, name: &[u8], kind: FileKind, mode: u32) -> Result<u32, i32> {
    if name.len() == 0 || name.len() > NAME_MAX {
        return Err(EINVAL);
    }
    if lookup(parent, name).is_some() {
        return Err(EEXIST);
    }
    let parent_node = inode(parent).ok_or(ENOENT)?;
    if parent_node.kind != FileKind::Directory {
        return Err(ENOTDIR);
    }
    let inum = alloc_inode(kind, mode, parent)?;
    add_dirent(parent, inum, name)?;
    if kind == FileKind::Directory {
        fs().inodes[parent as usize].nlink = fs().inodes[parent as usize].nlink.saturating_add(1);
    }
    Ok(inum)
}

fn alloc_inode(kind: FileKind, mode: u32, parent: u32) -> Result<u32, i32> {
    let fs = fs();
    let mut i = 1usize;
    while i < MAX_INODES {
        if !fs.inodes[i].used {
            fs.inodes[i] = Inode {
                used: true,
                kind,
                nlink: if kind == FileKind::Directory { 2 } else { 1 },
                refs: 0,
                _mode: mode,
                size: 0,
                parent,
                src: InodeData::None,
            };
            return Ok(i as u32);
        }
        i += 1;
    }
    Err(ENOSPC)
}

fn add_dirent(parent: u32, child: u32, name: &[u8]) -> Result<(), i32> {
    let fs = fs();
    let mut i = 0usize;
    while i < MAX_DIR_ENTRIES {
        if !fs.dirents[i].used {
            let mut dent = Dirent::empty();
            dent.used = true;
            dent.parent = parent;
            dent.child = child;
            dent.name[..name.len()].copy_from_slice(name);
            dent.name_len = name.len() as u8;
            fs.dirents[i] = dent;
            return Ok(());
        }
        i += 1;
    }
    Err(ENOSPC)
}

pub(crate) fn ensure_dir(path: &[u8]) -> Result<u32, i32> {
    match walk(path) {
        Ok(inum) => {
            let node = inode(inum).ok_or(ENOENT)?;
            if node.kind != FileKind::Directory {
                return Err(ENOTDIR);
            }
            Ok(inum)
        }
        Err(ENOENT) => mkdir(path),
        Err(e) => Err(e),
    }
}

pub(crate) fn install_console() -> Result<u32, i32> {
    if let Ok(inum) = walk(b"/dev/console") {
        return Ok(inum);
    }
    ensure_dir(b"/dev")?;
    let inum = CONSOLE_INO;
    if (inum as usize) < MAX_INODES && !fs().inodes[inum as usize].used {
        fs().inodes[inum as usize] = Inode {
            used: true,
            kind: FileKind::Device,
            nlink: 1,
            refs: 0,
            _mode: S_IFCHR | 0o666,
            size: 0,
            parent: walk(b"/dev").unwrap_or(ROOT_INO),
            src: InodeData::None,
        };
        add_dirent(walk(b"/dev").unwrap_or(ROOT_INO), inum, b"console")?;
        return Ok(inum);
    }
    create_file(
        walk(b"/dev").unwrap_or(ROOT_INO),
        b"console",
        FileKind::Device,
        S_IFCHR | 0o666,
    )
}

pub(crate) fn embed_file(path: &[u8], offset: usize, len: usize, mode: u32) -> Result<u32, i32> {
    if let Ok(inum) = walk(path) {
        return Ok(inum);
    }
    let (parent, name) = split_parent(path).ok_or(ENOENT)?;
    if name.is_empty() {
        return Ok(parent);
    }
    if !fs().inodes[parent as usize].used {
        return Err(ENOENT);
    }
    // Create missing parent dirs as we unpack.
    let inum = create_file(parent, name, FileKind::File, mode | S_IFREG)?;
    fs().inodes[inum as usize].size = len;
    fs().inodes[inum as usize].src = InodeData::Embedded { offset, len };
    Ok(inum)
}

pub(crate) fn embed_dir(path: &[u8]) -> Result<u32, i32> {
    ensure_dir(path)
}

pub(crate) fn read_inode(inum: u32, offset: usize, dst: &mut [u8]) -> Result<usize, i32> {
    let node = inode(inum).ok_or(ENOENT)?;
    if offset >= node.size {
        return Ok(0);
    }
    let n = min(dst.len(), node.size - offset);
    match node.src {
        InodeData::Embedded { offset: base, .. } => {
            let src = crate::rootfs_bytes();
            let start = base + offset;
            dst[..n].copy_from_slice(&src[start..start + n]);
            Ok(n)
        }
        InodeData::Pages { pages, n: npages } => {
            let mut done = 0usize;
            while done < n {
                let pos = offset + done;
                let page_idx = pos / PAGE;
                if page_idx >= npages as usize {
                    break;
                }
                let page = pages[page_idx] as usize;
                let inner = pos % PAGE;
                let chunk = min(n - done, PAGE - inner);
                dst[done..done + chunk].copy_from_slice(&fs().pages[page][inner..inner + chunk]);
                done += chunk;
            }
            Ok(done)
        }
        InodeData::None => Ok(0),
    }
}

pub(crate) fn write_inode(inum: u32, offset: usize, src: &[u8]) -> Result<usize, i32> {
    make_writable(inum)?;
    let fs = fs();
    let node = &mut fs.inodes[inum as usize];
    if !node.used || node.kind != FileKind::File {
        return Err(EISDIR);
    }
    let end = offset.saturating_add(src.len());
    if end > node.size {
        node.size = end;
    }
    let InodeData::Pages { pages, n: npages } = node.src else {
        return Err(ENOMEM);
    };
    let mut pages = pages;
    let mut npages = npages;
    let mut done = 0usize;
    while done < src.len() {
        let pos = offset + done;
        let page_idx = pos / PAGE;
        if page_idx >= MAX_FILE_PAGES {
            return Err(ENOSPC);
        }
        while page_idx >= npages as usize {
            let page = alloc_page()?;
            pages[npages as usize] = page as u16;
            npages += 1;
            fs.inodes[inum as usize].src = InodeData::Pages { pages, n: npages };
        }
        let page = pages[page_idx] as usize;
        let inner = pos % PAGE;
        let chunk = min(src.len() - done, PAGE - inner);
        fs.pages[page][inner..inner + chunk].copy_from_slice(&src[done..done + chunk]);
        done += chunk;
    }
    Ok(done)
}

fn make_writable(inum: u32) -> Result<(), i32> {
    let node = inode(inum).ok_or(ENOENT)?;
    match node.src {
        InodeData::Pages { .. } | InodeData::None => {
            if matches!(node.src, InodeData::None) {
                fs().inodes[inum as usize].src = InodeData::Pages {
                    pages: [0; MAX_FILE_PAGES],
                    n: 0,
                };
            }
            Ok(())
        }
        InodeData::Embedded { offset, len } => {
            let mut tmp = [0u8; 512];
            let mut copied = 0usize;
            fs().inodes[inum as usize].src = InodeData::Pages {
                pages: [0; MAX_FILE_PAGES],
                n: 0,
            };
            fs().inodes[inum as usize].size = 0;
            while copied < len {
                let chunk = min(len - copied, tmp.len());
                let src = crate::rootfs_bytes();
                tmp[..chunk].copy_from_slice(&src[offset + copied..offset + copied + chunk]);
                write_inode(inum, copied, &tmp[..chunk])?;
                copied += chunk;
            }
            Ok(())
        }
    }
}

fn truncate(inum: u32) -> Result<(), i32> {
    free_inode_data(inum);
    let node = &mut fs().inodes[inum as usize];
    node.size = 0;
    node.src = InodeData::Pages {
        pages: [0; MAX_FILE_PAGES],
        n: 0,
    };
    Ok(())
}

fn free_inode_data(inum: u32) {
    let fs = fs();
    if let InodeData::Pages { pages, n } = fs.inodes[inum as usize].src {
        let mut i = 0u16;
        while i < n {
            let page = pages[i as usize] as usize;
            if page < MAX_DATA_PAGES {
                fs.page_used[page] = false;
            }
            i += 1;
        }
    }
    fs.inodes[inum as usize].src = InodeData::None;
    fs.inodes[inum as usize].size = 0;
}

fn alloc_page() -> Result<usize, i32> {
    let fs = fs();
    let mut i = 0usize;
    while i < MAX_DATA_PAGES {
        if !fs.page_used[i] {
            fs.page_used[i] = true;
            fs.pages[i] = [0; PAGE];
            return Ok(i);
        }
        i += 1;
    }
    Err(ENOSPC)
}

pub(crate) fn seek(inum: u32, current: usize, offset: i64, whence: u64) -> Result<usize, i32> {
    let node = inode(inum).ok_or(ENOENT)?;
    let base = match whence as u32 {
        SEEK_SET => 0isize,
        SEEK_CUR => current as isize,
        SEEK_END => node.size as isize,
        _ => return Err(EINVAL),
    };
    let next = base.saturating_add(offset as isize);
    if next < 0 {
        return Err(EINVAL);
    }
    Ok(next as usize)
}

pub(crate) fn path_from_words(
    words: &[u64],
    start: usize,
    len: usize,
) -> Option<[u8; MAX_PATH_BYTES]> {
    if len == 0 || len > MAX_PATH_BYTES {
        return None;
    }
    let mut out = [0u8; MAX_PATH_BYTES];
    let mut i = 0usize;
    while i < len {
        out[i] = ((words[start + i / 8] >> ((i % 8) * 8)) & 0xff) as u8;
        i += 1;
    }
    Some(out)
}
