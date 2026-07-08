use core::cell::UnsafeCell;
use core::sync::atomic::{AtomicBool, AtomicUsize};

use xv6_abi::{FS_BLOCK_SIZE, XV6_FS_NDIRECT, Xv6Superblock};

pub(crate) const DINODE_SIZE: usize = 64;
pub(crate) const DINODES_PER_BLOCK: u32 = (FS_BLOCK_SIZE / DINODE_SIZE) as u32;
pub(crate) const BPB: u32 = (FS_BLOCK_SIZE * 8) as u32;
pub(crate) const FS_MAX_TRACKED_INODES: usize = 512;
pub(crate) const XV6_LOG_MAX_BLOCKS: usize = 30;
pub(crate) const FS_BLOCK_CACHE_CAP: usize = 16;

#[derive(Copy, Clone)]
pub(crate) struct FsState {
    pub(crate) ready: bool,
    pub(crate) superblock: Xv6Superblock,
}

impl FsState {
    pub(crate) const fn empty() -> Self {
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

pub(crate) struct FsStateCell {
    state: UnsafeCell<FsState>,
}

// xv6fs-server initializes filesystem geometry once and then reads snapshots
// from the single cooperative request loop.
unsafe impl Sync for FsStateCell {}

impl FsStateCell {
    const fn new() -> Self {
        Self {
            state: UnsafeCell::new(FsState::empty()),
        }
    }

    pub(crate) fn get(&self) -> FsState {
        unsafe { *self.state.get() }
    }

    pub(crate) fn set(&self, state: FsState) {
        unsafe {
            *self.state.get() = state;
        }
    }

    pub(crate) fn ready(&self) -> bool {
        self.get().ready
    }
}

#[derive(Copy, Clone)]
pub(crate) struct Dinode {
    pub(crate) typ: i16,
    pub(crate) major: u16,
    pub(crate) minor: u16,
    pub(crate) nlink: u16,
    pub(crate) size: u32,
    pub(crate) addrs: [u32; XV6_FS_NDIRECT + 1],
}

#[derive(Copy, Clone)]
pub(crate) struct DirEntryLoc {
    pub(crate) inum: u32,
    pub(crate) blockno: u32,
    pub(crate) offset: usize,
}

pub(crate) static FS_STATE: FsStateCell = FsStateCell::new();
pub(crate) static LOG_ACTIVE: AtomicBool = AtomicBool::new(false);
pub(crate) static LOG_LEN: AtomicUsize = AtomicUsize::new(0);
