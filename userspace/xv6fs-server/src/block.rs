use core::cell::UnsafeCell;
use core::future::Future;
use core::sync::atomic::{AtomicUsize, Ordering, fence};

use sel4_user::{info, read_u32, rt, warn, write_u32};
use xv6_abi::{DiskRequestOp, FS_BLOCK_SIZE, Xv6Status, Xv6Superblock};

use crate::disk::{
    DiskSharedSlot, copy_shared_block_from_ptr, disk_data_request, disk_data_request_and_wait,
    flush_disk, flush_disk_async,
};
pub(crate) use crate::disk::{
    copy_shared_block_to_ptr, host_shared_write_buffer, shared_block, with_shared_block_mut,
};
use crate::types::{
    FS_BLOCK_CACHE_CAP, FS_STATE, FsState, LOG_ACTIVE, LOG_LEN, XV6_LOG_MAX_BLOCKS,
};

struct BlockRuntimeState {
    log_blocknos: [u32; XV6_LOG_MAX_BLOCKS],
    log_blocks: [[u8; FS_BLOCK_SIZE]; XV6_LOG_MAX_BLOCKS],
    cache_valid: [bool; FS_BLOCK_CACHE_CAP],
    cache_blocknos: [u32; FS_BLOCK_CACHE_CAP],
    cache_ages: [u64; FS_BLOCK_CACHE_CAP],
    cache_data: [[u8; FS_BLOCK_SIZE]; FS_BLOCK_CACHE_CAP],
    cache_clock: u64,
}

impl BlockRuntimeState {
    const fn new() -> Self {
        Self {
            log_blocknos: [0; XV6_LOG_MAX_BLOCKS],
            log_blocks: [[0; FS_BLOCK_SIZE]; XV6_LOG_MAX_BLOCKS],
            cache_valid: [false; FS_BLOCK_CACHE_CAP],
            cache_blocknos: [0; FS_BLOCK_CACHE_CAP],
            cache_ages: [0; FS_BLOCK_CACHE_CAP],
            cache_data: [[0; FS_BLOCK_SIZE]; FS_BLOCK_CACHE_CAP],
            cache_clock: 1,
        }
    }
}

struct BlockRuntime {
    state: UnsafeCell<BlockRuntimeState>,
}

// xv6fs-server handles one filesystem request at a time; transaction log and
// block-cache mutation are serialized by that cooperative control flow.
unsafe impl Sync for BlockRuntime {}

impl BlockRuntime {
    const fn new() -> Self {
        Self {
            state: UnsafeCell::new(BlockRuntimeState::new()),
        }
    }

    fn log_blockno(&self, index: usize) -> u32 {
        unsafe { (&*self.state.get()).log_blocknos[index] }
    }

    fn set_log_blockno(&self, index: usize, blockno: u32) {
        unsafe {
            (&mut *self.state.get()).log_blocknos[index] = blockno;
        }
    }

    unsafe fn log_block_ptr(&self, index: usize) -> *mut u8 {
        unsafe {
            (&mut *self.state.get())
                .log_blocks
                .as_mut_ptr()
                .cast::<u8>()
                .add(index * FS_BLOCK_SIZE)
        }
    }

    fn cached_entry(&self, slot: usize) -> (bool, u32) {
        let state = unsafe { &*self.state.get() };
        (state.cache_valid[slot], state.cache_blocknos[slot])
    }

    fn set_cached_block(&self, slot: usize, blockno: u32) {
        let state = unsafe { &mut *self.state.get() };
        state.cache_blocknos[slot] = blockno;
        state.cache_valid[slot] = true;
    }

    fn invalidate_cached_slot(&self, slot: usize) {
        unsafe {
            (&mut *self.state.get()).cache_valid[slot] = false;
        }
    }

    fn cache_age(&self, slot: usize) -> u64 {
        unsafe { (&*self.state.get()).cache_ages[slot] }
    }

    fn touch_cached_block(&self, slot: usize) {
        let state = unsafe { &mut *self.state.get() };
        let clock = state.cache_clock;
        let next = clock.wrapping_add(1).max(1);
        state.cache_clock = next;
        state.cache_ages[slot] = clock;
    }

    fn cache_data_ptr(&self, slot: usize) -> *mut u8 {
        unsafe {
            (&mut *self.state.get())
                .cache_data
                .as_mut_ptr()
                .cast::<u8>()
                .add(slot * FS_BLOCK_SIZE)
        }
    }
}

static BLOCK_RUNTIME: BlockRuntime = BlockRuntime::new();

static READ_ACTIVE: AtomicUsize = AtomicUsize::new(0);
static TX_WAITERS: AtomicUsize = AtomicUsize::new(0);

fn increment_saturating(counter: &AtomicUsize) {
    let _ = counter.fetch_update(Ordering::Relaxed, Ordering::Relaxed, |value| {
        Some(value.saturating_add(1))
    });
}

fn decrement_saturating(counter: &AtomicUsize) {
    let _ = counter.fetch_update(Ordering::Relaxed, Ordering::Relaxed, |value| {
        Some(value.saturating_sub(1))
    });
}

pub(crate) fn handle_transactional<F>(op: F) -> [u64; 4]
where
    F: FnOnce() -> [u64; 4],
{
    if !begin_transaction() {
        return [Xv6Status::InvalidArgument.raw(), 0, 0, 0];
    }
    let reply = op();
    if reply[0] != Xv6Status::Ok.raw() {
        abort_transaction();
        return reply;
    }
    if !commit_transaction() {
        abort_transaction();
        return [Xv6Status::InvalidArgument.raw(), 0, 0, 0];
    }
    reply
}

pub(crate) async fn handle_transactional_async<F>(op: F) -> [u64; 4]
where
    F: FnOnce() -> [u64; 4],
{
    begin_transaction_async().await;
    let reply = op();
    if reply[0] != Xv6Status::Ok.raw() {
        abort_transaction();
        return reply;
    }
    if !commit_transaction_async().await {
        abort_transaction();
        return [Xv6Status::InvalidArgument.raw(), 0, 0, 0];
    }
    reply
}

pub(crate) async fn handle_transactional_future<F>(future: F) -> [u64; 4]
where
    F: Future<Output = [u64; 4]>,
{
    begin_transaction_async().await;
    let reply = future.await;
    if reply[0] != Xv6Status::Ok.raw() {
        abort_transaction();
        return reply;
    }
    if !commit_transaction_async().await {
        abort_transaction();
        return [Xv6Status::InvalidArgument.raw(), 0, 0, 0];
    }
    reply
}

fn begin_transaction() -> bool {
    if LOG_ACTIVE.load(Ordering::Relaxed) || READ_ACTIVE.load(Ordering::Relaxed) != 0 {
        warn!("xv6fs-server: nested transaction");
        return false;
    }
    LOG_ACTIVE.store(true, Ordering::Relaxed);
    LOG_LEN.store(0, Ordering::Relaxed);
    true
}

pub(crate) async fn begin_read_only() -> ReadOnlyGuard {
    loop {
        if !LOG_ACTIVE.load(Ordering::Relaxed) && TX_WAITERS.load(Ordering::Relaxed) == 0 {
            increment_saturating(&READ_ACTIVE);
            break;
        }
        rt::yield_now().await;
    }
    ReadOnlyGuard { active: true }
}

pub(crate) struct ReadOnlyGuard {
    active: bool,
}

impl Drop for ReadOnlyGuard {
    fn drop(&mut self) {
        if self.active {
            decrement_saturating(&READ_ACTIVE);
            self.active = false;
        }
    }
}

fn abort_transaction() {
    LOG_LEN.store(0, Ordering::Relaxed);
    LOG_ACTIVE.store(false, Ordering::Relaxed);
}

async fn begin_transaction_async() {
    let mut registered = false;
    loop {
        if LOG_ACTIVE.load(Ordering::Relaxed) || READ_ACTIVE.load(Ordering::Relaxed) != 0 {
            if !registered {
                increment_saturating(&TX_WAITERS);
                registered = true;
            }
        } else {
            if registered {
                decrement_saturating(&TX_WAITERS);
            }
            LOG_ACTIVE.store(true, Ordering::Relaxed);
            LOG_LEN.store(0, Ordering::Relaxed);
            return;
        }
        rt::yield_now().await;
    }
}

fn commit_transaction() -> bool {
    let len = LOG_LEN.load(Ordering::Relaxed);
    if len == 0 {
        abort_transaction();
        return true;
    }

    let state = FS_STATE.get();
    let capacity = log_capacity(&state);
    if !state.ready || len > capacity {
        return false;
    }

    let mut i = 0usize;
    while i < len {
        copy_shared_block_from_ptr(unsafe { log_block_ptr(i) as *const u8 });
        fence(Ordering::SeqCst);
        if !write_disk_block_raw(state.superblock.logstart + 1 + i as u32) {
            return false;
        }
        i += 1;
    }

    if !flush_disk() {
        return false;
    }
    if !write_log_header(len) {
        return false;
    }
    if !flush_disk() {
        return false;
    }

    i = 0;
    while i < len {
        copy_shared_block_from_ptr(unsafe { log_block_ptr(i) as *const u8 });
        fence(Ordering::SeqCst);
        if !write_disk_block_raw(log_blockno(i)) {
            return false;
        }
        i += 1;
    }

    if !flush_disk() {
        return false;
    }
    if !write_log_header(0) {
        return false;
    }
    if !flush_disk() {
        return false;
    }
    abort_transaction();
    true
}

async fn commit_transaction_async() -> bool {
    let len = LOG_LEN.load(Ordering::Relaxed);
    if len == 0 {
        abort_transaction();
        return true;
    }

    let state = FS_STATE.get();
    let capacity = log_capacity(&state);
    if !state.ready || len > capacity {
        return false;
    }

    let mut i = 0usize;
    while i < len {
        copy_shared_block_from_ptr(unsafe { log_block_ptr(i) as *const u8 });
        fence(Ordering::SeqCst);
        if !write_disk_block_raw_async(state.superblock.logstart + 1 + i as u32).await {
            return false;
        }
        i += 1;
    }

    if !flush_disk_async().await {
        return false;
    }
    if !write_log_header_async(len).await {
        return false;
    }
    if !flush_disk_async().await {
        return false;
    }

    i = 0;
    while i < len {
        copy_shared_block_from_ptr(unsafe { log_block_ptr(i) as *const u8 });
        fence(Ordering::SeqCst);
        if !write_disk_block_raw_async(log_blockno(i)).await {
            return false;
        }
        i += 1;
    }

    if !flush_disk_async().await {
        return false;
    }
    if !write_log_header_async(0).await {
        return false;
    }
    if !flush_disk_async().await {
        return false;
    }
    abort_transaction();
    true
}

pub(crate) fn recover_log() -> bool {
    let state = FS_STATE.get();
    if !state.ready {
        return false;
    }
    let Some(len) = read_log_header() else {
        return false;
    };
    if len > log_capacity(&state) {
        warn!("xv6fs-server: invalid log length={}", len);
        return false;
    }
    if len != 0 {
        info!("xv6fs-server: recovering log blocks={}", len);
    }

    let mut i = 0usize;
    while i < len {
        let dst = log_blockno(i);
        if dst >= state.superblock.size
            || !read_disk_block_raw(state.superblock.logstart + 1 + i as u32)
        {
            return false;
        }
        fence(Ordering::SeqCst);
        if !write_disk_block_raw(dst) {
            return false;
        }
        i += 1;
    }
    if !flush_disk() {
        return false;
    }
    write_log_header(0) && flush_disk()
}

fn read_log_header() -> Option<usize> {
    let state = FS_STATE.get();
    if !state.ready || !read_disk_block_raw(state.superblock.logstart) {
        return None;
    }
    fence(Ordering::SeqCst);
    let block = shared_block();
    let len = read_u32(block, 0) as usize;
    if len > log_capacity(&state) {
        return None;
    }
    let mut i = 0usize;
    while i < len {
        let blockno = read_u32(block, 4 + i * 4);
        if blockno >= state.superblock.size {
            return None;
        }
        set_log_blockno(i, blockno);
        i += 1;
    }
    LOG_LEN.store(len, Ordering::Relaxed);
    Some(len)
}

fn write_log_header(len: usize) -> bool {
    let state = FS_STATE.get();
    if !state.ready || len > log_capacity(&state) {
        return false;
    }
    with_shared_block_mut(|block| {
        let mut i = 0usize;
        while i < FS_BLOCK_SIZE {
            block[i] = 0;
            i += 1;
        }
        write_u32(block, 0, len as u32);
        i = 0;
        while i < len {
            write_u32(block, 4 + i * 4, log_blockno(i));
            i += 1;
        }
    });
    fence(Ordering::SeqCst);
    write_disk_block_raw(state.superblock.logstart)
}

async fn write_log_header_async(len: usize) -> bool {
    let state = FS_STATE.get();
    if !state.ready || len > log_capacity(&state) {
        return false;
    }
    with_shared_block_mut(|block| {
        let mut i = 0usize;
        while i < FS_BLOCK_SIZE {
            block[i] = 0;
            i += 1;
        }
        write_u32(block, 0, len as u32);
        i = 0;
        while i < len {
            write_u32(block, 4 + i * 4, log_blockno(i));
            i += 1;
        }
    });
    fence(Ordering::SeqCst);
    write_disk_block_raw_async(state.superblock.logstart).await
}

fn log_write_shared(blockno: u32) -> bool {
    let state = FS_STATE.get();
    if !state.ready || blockno >= state.superblock.size {
        return false;
    }
    if blockno >= state.superblock.logstart
        && blockno
            < state
                .superblock
                .logstart
                .saturating_add(state.superblock.nlog)
    {
        warn!("xv6fs-server: refusing to journal log block={}", blockno);
        return false;
    }

    let index = if let Some(index) = find_logged_block(blockno) {
        index
    } else {
        let len = LOG_LEN.load(Ordering::Relaxed);
        if len >= log_capacity(&state) {
            warn!("xv6fs-server: transaction too large");
            return false;
        }
        set_log_blockno(len, blockno);
        LOG_LEN.store(len + 1, Ordering::Relaxed);
        len
    };

    fence(Ordering::SeqCst);
    copy_shared_block_to_ptr(unsafe { log_block_ptr(index) });
    invalidate_cached_block(blockno);
    true
}

fn find_logged_block(blockno: u32) -> Option<usize> {
    let len = LOG_LEN.load(Ordering::Relaxed);
    let mut i = 0usize;
    while i < len {
        if log_blockno(i) == blockno {
            return Some(i);
        }
        i += 1;
    }
    None
}

fn log_capacity(state: &FsState) -> usize {
    core::cmp::min(
        state.superblock.nlog.saturating_sub(1) as usize,
        XV6_LOG_MAX_BLOCKS,
    )
}

fn log_blockno(index: usize) -> u32 {
    BLOCK_RUNTIME.log_blockno(index)
}

fn set_log_blockno(index: usize, blockno: u32) {
    BLOCK_RUNTIME.set_log_blockno(index, blockno);
}

unsafe fn log_block_ptr(index: usize) -> *mut u8 {
    unsafe { BLOCK_RUNTIME.log_block_ptr(index) }
}

fn load_cached_block(blockno: u32) -> bool {
    let mut i = 0usize;
    while i < FS_BLOCK_CACHE_CAP {
        let (valid, cached_blockno) = BLOCK_RUNTIME.cached_entry(i);
        if valid && cached_blockno == blockno {
            copy_shared_block_from_ptr(block_cache_data_ptr(i) as *const u8);
            touch_cached_block(i);
            fence(Ordering::SeqCst);
            return true;
        }
        i += 1;
    }
    false
}

fn store_cached_block(blockno: u32) {
    let slot = select_cache_slot(blockno);
    fence(Ordering::SeqCst);
    copy_shared_block_to_ptr(block_cache_data_ptr(slot));
    BLOCK_RUNTIME.set_cached_block(slot, blockno);
    touch_cached_block(slot);
}

fn invalidate_cached_block(blockno: u32) {
    let mut i = 0usize;
    while i < FS_BLOCK_CACHE_CAP {
        let (valid, cached_blockno) = BLOCK_RUNTIME.cached_entry(i);
        if valid && cached_blockno == blockno {
            BLOCK_RUNTIME.invalidate_cached_slot(i);
        }
        i += 1;
    }
}

fn select_cache_slot(blockno: u32) -> usize {
    let mut oldest_slot = 0usize;
    let mut oldest_age = u64::MAX;
    let mut i = 0usize;
    while i < FS_BLOCK_CACHE_CAP {
        let (valid, cached_blockno) = BLOCK_RUNTIME.cached_entry(i);
        if !valid || cached_blockno == blockno {
            return i;
        }
        let age = BLOCK_RUNTIME.cache_age(i);
        if age < oldest_age {
            oldest_age = age;
            oldest_slot = i;
        }
        i += 1;
    }
    oldest_slot
}

fn touch_cached_block(slot: usize) {
    BLOCK_RUNTIME.touch_cached_block(slot);
}

fn block_cache_data_ptr(slot: usize) -> *mut u8 {
    BLOCK_RUNTIME.cache_data_ptr(slot)
}

pub(crate) fn read_disk_block(blockno: u32) -> bool {
    if LOG_ACTIVE.load(Ordering::Relaxed) {
        if let Some(index) = find_logged_block(blockno) {
            copy_shared_block_from_ptr(unsafe { log_block_ptr(index) as *const u8 });
            fence(Ordering::SeqCst);
            return true;
        }
    }
    if load_cached_block(blockno) {
        return true;
    }
    read_disk_block_raw(blockno)
}

pub(crate) async fn read_disk_block_async(blockno: u32) -> bool {
    if LOG_ACTIVE.load(Ordering::Relaxed) {
        if let Some(index) = find_logged_block(blockno) {
            copy_shared_block_from_ptr(unsafe { log_block_ptr(index) as *const u8 });
            fence(Ordering::SeqCst);
            return true;
        }
    }
    if load_cached_block(blockno) {
        return true;
    }
    read_disk_block_raw_async(blockno).await
}

fn read_disk_block_raw(blockno: u32) -> bool {
    let reply = disk_data_request_and_wait(DiskRequestOp::Read.raw(), blockno);
    if reply[0] != Xv6Status::Ok.raw() {
        warn!(
            "xv6fs-server: disk read failed block={} status={}",
            blockno, reply[0]
        );
        return false;
    }
    let ok = reply[1] == FS_BLOCK_SIZE as u64 && reply[2] == blockno as u64;
    if ok {
        store_cached_block(blockno);
    }
    ok
}

async fn read_disk_block_raw_async(blockno: u32) -> bool {
    let reply = disk_data_request(
        DiskRequestOp::Read.raw(),
        blockno,
        DiskSharedSlot::Main.value(),
    )
    .await;
    if reply[0] != Xv6Status::Ok.raw() {
        warn!(
            "xv6fs-server: disk read failed block={} status={}",
            blockno, reply[0]
        );
        return false;
    }
    let ok = reply[1] == FS_BLOCK_SIZE as u64 && reply[2] == blockno as u64;
    if ok {
        store_cached_block(blockno);
    }
    ok
}

pub(crate) fn write_disk_block(blockno: u32) -> bool {
    if LOG_ACTIVE.load(Ordering::Relaxed) {
        return log_write_shared(blockno);
    }
    write_disk_block_raw(blockno)
}

pub(crate) async fn write_disk_block_async(blockno: u32) -> bool {
    if LOG_ACTIVE.load(Ordering::Relaxed) {
        return log_write_shared(blockno);
    }
    write_disk_block_raw_async(blockno).await
}

fn write_disk_block_raw(blockno: u32) -> bool {
    invalidate_cached_block(blockno);
    let reply = disk_data_request_and_wait(DiskRequestOp::Write.raw(), blockno);
    if reply[0] != Xv6Status::Ok.raw() {
        warn!(
            "xv6fs-server: disk write failed block={} status={}",
            blockno, reply[0]
        );
        return false;
    }
    let ok = reply[1] == FS_BLOCK_SIZE as u64 && reply[2] == blockno as u64;
    if ok {
        store_cached_block(blockno);
    }
    ok
}

async fn write_disk_block_raw_async(blockno: u32) -> bool {
    invalidate_cached_block(blockno);
    let reply = disk_data_request(
        DiskRequestOp::Write.raw(),
        blockno,
        DiskSharedSlot::Main.value(),
    )
    .await;
    if reply[0] != Xv6Status::Ok.raw() {
        warn!(
            "xv6fs-server: disk write failed block={} status={}",
            blockno, reply[0]
        );
        return false;
    }
    let ok = reply[1] == FS_BLOCK_SIZE as u64 && reply[2] == blockno as u64;
    if ok {
        store_cached_block(blockno);
    }
    ok
}

pub(crate) fn exercise_disk_write(blockno: u32) -> bool {
    let mut backup = [0u8; FS_BLOCK_SIZE];
    if !read_disk_block(blockno) {
        return false;
    }
    fence(Ordering::SeqCst);
    copy_shared_block_to_ptr(backup.as_mut_ptr());
    with_shared_block_mut(|block| {
        let mut i = 0usize;
        while i < FS_BLOCK_SIZE {
            let byte = (i as u8).wrapping_mul(31).wrapping_add(0xa5);
            block[i] = byte;
            i += 1;
        }
    });
    fence(Ordering::SeqCst);
    let wrote_pattern = write_disk_block(blockno);
    let verified = wrote_pattern && read_disk_block(blockno) && verify_write_pattern();
    copy_shared_block_from_ptr(backup.as_ptr());
    fence(Ordering::SeqCst);
    let restored = write_disk_block(blockno);
    if verified && restored {
        info!("xv6fs-server: disk write verified block={}", blockno);
        true
    } else {
        false
    }
}

fn verify_write_pattern() -> bool {
    fence(Ordering::SeqCst);
    let block = shared_block();
    let mut i = 0usize;
    while i < FS_BLOCK_SIZE {
        let expected = (i as u8).wrapping_mul(31).wrapping_add(0xa5);
        if block[i] != expected {
            return false;
        }
        i += 1;
    }
    true
}

pub(crate) fn read_superblock_from_shared() -> Xv6Superblock {
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
