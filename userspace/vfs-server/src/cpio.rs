use linux_abi::{S_IFDIR, S_IFMT, S_IFREG};

use crate::ramfs;

const HEADER_LEN: usize = 110;

pub(crate) fn unpack(blob: &[u8]) -> bool {
    let mut off = 0usize;
    while off + HEADER_LEN <= blob.len() {
        if &blob[off..off + 6] != b"070701" {
            return false;
        }
        let mode = match parse_hex(&blob[off + 14..off + 22]) {
            Some(v) => v,
            None => return false,
        };
        let filesize = match parse_hex(&blob[off + 54..off + 62]) {
            Some(v) => v as usize,
            None => return false,
        };
        let namesize = match parse_hex(&blob[off + 94..off + 102]) {
            Some(v) => v as usize,
            None => return false,
        };
        let name_off = off + HEADER_LEN;
        if name_off + namesize > blob.len() {
            return false;
        }
        let name = &blob[name_off..name_off + namesize];
        let name_len = name.iter().position(|&b| b == 0).unwrap_or(namesize);
        let name = &name[..name_len];
        let data_off = align4(name_off + namesize);
        if data_off + filesize > blob.len() {
            return false;
        }
        if name == b"TRAILER!!!" {
            return true;
        }
        if name != b"." && !name.is_empty() {
            let mut path = [0u8; linux_abi::MAX_PATH_BYTES];
            path[0] = b'/';
            let n = core::cmp::min(name.len(), linux_abi::MAX_PATH_BYTES - 1);
            path[1..1 + n].copy_from_slice(&name[..n]);
            let path = &path[..n + 1];
            ensure_parents(path);
            let kind = mode & S_IFMT;
            if kind == S_IFDIR || name.last() == Some(&b'/') {
                if ramfs::embed_dir(path).is_err() {
                    return false;
                }
            } else if kind == 0 || kind == S_IFREG {
                if ramfs::embed_file(path, data_off, filesize, mode).is_err() {
                    return false;
                }
            }
        }
        off = align4(data_off + filesize);
    }
    true
}

fn ensure_parents(path: &[u8]) {
    let mut i = 1usize;
    while i < path.len() {
        if path[i] == b'/' && i > 1 {
            let _ = ramfs::embed_dir(&path[..i]);
        }
        i += 1;
    }
}

fn parse_hex(bytes: &[u8]) -> Option<u32> {
    if bytes.len() != 8 {
        return None;
    }
    let mut v = 0u32;
    for &b in bytes {
        v <<= 4;
        v |= match b {
            b'0'..=b'9' => (b - b'0') as u32,
            b'a'..=b'f' => (b - b'a' + 10) as u32,
            b'A'..=b'F' => (b - b'A' + 10) as u32,
            _ => return None,
        };
    }
    Some(v)
}

fn align4(n: usize) -> usize {
    (n + 3) & !3
}
