use core::cell::UnsafeCell;
use core::cmp::min;
use core::sync::atomic::{AtomicU64, Ordering, fence};

use crate::arch::current as arch;
use sel4_user::{
    IpcMessage, call_checked, msg_info, msg_label, msg_len, sel4_call, sel4_send, sel4_yield,
};

use crate::allocator::Allocator;
use crate::child::{copy_from_child, copy_to_child, elf_image_valid};
use crate::consts::*;
use crate::types::TaskStruct;
use crate::util::{halt_loop, warn, write_i32, write_u16, write_u32, write_u64_bytes};

static VFS_SERVER_EP: AtomicU64 = AtomicU64::new(0);
static NEXT_VFS_REQUEST_ID: AtomicU64 = AtomicU64::new(1);
static DEFERRED_REPLY_OVERRIDE: AtomicU64 = AtomicU64::new(0);

const FS_BUSY_RETRY_LIMIT: usize = 4096;
const VFS_ASYNC_REQUEST_CAP: usize = 16;
const VFS_ASYNC_STATUS: u8 = 0;
const VFS_ASYNC_OPEN: u8 = 1;
const VFS_ASYNC_CHDIR: u8 = 2;
const VFS_ASYNC_CLOSE: u8 = 3;
const VFS_ASYNC_DUP: u8 = 4;
const VFS_ASYNC_FSTAT: u8 = 5;
const VFS_ASYNC_PIPE: u8 = 6;
const VFS_ASYNC_READ: u8 = 7;
const VFS_ASYNC_WRITE: u8 = 8;

pub(crate) fn init_vfs_client(vfs_ep: u64) {
    VFS_SERVER_EP.store(vfs_ep, Ordering::Relaxed);
}

#[derive(Copy, Clone)]
struct VfsAsyncRequest {
    active: bool,
    request_id: u64,
    pid: u64,
    reply_slot: u64,
    reply_mrs: arch::FaultReplyFrame,
    kind: u8,
    fd: usize,
    fd2: usize,
    user_ptr: u64,
    len: usize,
    done: usize,
    cwd: [u8; MAX_PATH_BYTES],
    cwd_len: usize,
}

impl VfsAsyncRequest {
    const fn empty() -> Self {
        Self {
            active: false,
            request_id: 0,
            pid: 0,
            reply_slot: 0,
            reply_mrs: [0; arch::FAULT_REPLY_WORDS],
            kind: VFS_ASYNC_STATUS,
            fd: 0,
            fd2: 0,
            user_ptr: 0,
            len: 0,
            done: 0,
            cwd: [0; MAX_PATH_BYTES],
            cwd_len: 0,
        }
    }
}

struct VfsClientState {
    exec_image_buf: [u8; MAX_FILE_BYTES],
    async_requests: [VfsAsyncRequest; VFS_ASYNC_REQUEST_CAP],
}

impl VfsClientState {
    const fn new() -> Self {
        Self {
            exec_image_buf: [0; MAX_FILE_BYTES],
            async_requests: [VfsAsyncRequest::empty(); VFS_ASYNC_REQUEST_CAP],
        }
    }
}

struct VfsClient {
    state: UnsafeCell<VfsClientState>,
}

// xv6-host drives VFS client state from the single rootserver event loop.
// Deferred VFS replies and exec image loads are serialized by that loop.
unsafe impl Sync for VfsClient {}

impl VfsClient {
    const fn new() -> Self {
        Self {
            state: UnsafeCell::new(VfsClientState::new()),
        }
    }

    fn set_async_request(&self, slot: usize, request: VfsAsyncRequest) {
        unsafe {
            (&mut *self.state.get()).async_requests[slot] = request;
        }
    }

    fn take_async_request(&self, slot: usize) -> VfsAsyncRequest {
        let state = unsafe { &mut *self.state.get() };
        let request = state.async_requests[slot];
        state.async_requests[slot] = VfsAsyncRequest::empty();
        request
    }

    fn has_active_async_requests(&self) -> bool {
        let state = unsafe { &*self.state.get() };
        let mut i = 0usize;
        while i < VFS_ASYNC_REQUEST_CAP {
            if state.async_requests[i].active {
                return true;
            }
            i += 1;
        }
        false
    }

    fn alloc_async_request(&self) -> Option<usize> {
        let state = unsafe { &*self.state.get() };
        let mut i = 0usize;
        while i < VFS_ASYNC_REQUEST_CAP {
            if !state.async_requests[i].active {
                return Some(i);
            }
            i += 1;
        }
        None
    }

    fn find_async_request(&self, request_id: u64) -> Option<usize> {
        let state = unsafe { &*self.state.get() };
        let mut i = 0usize;
        while i < VFS_ASYNC_REQUEST_CAP {
            let request = state.async_requests[i];
            if request.active && request.request_id == request_id {
                return Some(i);
            }
            i += 1;
        }
        None
    }

    fn exec_image_buf_ptr(&self) -> *mut u8 {
        unsafe { (&mut *self.state.get()).exec_image_buf.as_mut_ptr() }
    }

    fn exec_image(&self, len: usize) -> &'static [u8] {
        unsafe { core::slice::from_raw_parts((&*self.state.get()).exec_image_buf.as_ptr(), len) }
    }
}

static VFS_CLIENT: VfsClient = VfsClient::new();

pub(crate) fn start_vfs_status_request(
    alloc: &mut Allocator,
    child: &mut TaskStruct,
    syscall_mrs: &[u64; 64],
    label: u64,
    request_mrs: &[u64],
) -> crate::types::SyscallResult {
    start_vfs_async_request(
        alloc,
        child,
        syscall_mrs,
        label,
        request_mrs,
        VFS_ASYNC_STATUS,
        0,
        0,
        0,
        0,
        [0; MAX_PATH_BYTES],
        0,
    )
}

pub(crate) fn start_vfs_open_request(
    alloc: &mut Allocator,
    child: &mut TaskStruct,
    syscall_mrs: &[u64; 64],
    label: u64,
    request_mrs: &[u64],
    fd: usize,
) -> crate::types::SyscallResult {
    start_vfs_async_request(
        alloc,
        child,
        syscall_mrs,
        label,
        request_mrs,
        VFS_ASYNC_OPEN,
        fd,
        0,
        0,
        0,
        [0; MAX_PATH_BYTES],
        0,
    )
}

pub(crate) fn start_vfs_chdir_request(
    alloc: &mut Allocator,
    child: &mut TaskStruct,
    syscall_mrs: &[u64; 64],
    label: u64,
    request_mrs: &[u64],
    cwd: [u8; MAX_PATH_BYTES],
    cwd_len: usize,
) -> crate::types::SyscallResult {
    start_vfs_async_request(
        alloc,
        child,
        syscall_mrs,
        label,
        request_mrs,
        VFS_ASYNC_CHDIR,
        0,
        0,
        0,
        0,
        cwd,
        cwd_len,
    )
}

pub(crate) fn start_vfs_close_request(
    alloc: &mut Allocator,
    child: &mut TaskStruct,
    syscall_mrs: &[u64; 64],
    file: usize,
    fd: usize,
) -> crate::types::SyscallResult {
    start_vfs_async_request(
        alloc,
        child,
        syscall_mrs,
        VfsOp::Close.raw(),
        &[Xv6Protocol::HostToVfs.raw(), XV6_ABI_VERSION, file as u64],
        VFS_ASYNC_CLOSE,
        fd,
        0,
        0,
        0,
        [0; MAX_PATH_BYTES],
        0,
    )
}

pub(crate) fn start_vfs_dup_request(
    alloc: &mut Allocator,
    child: &mut TaskStruct,
    syscall_mrs: &[u64; 64],
    file: usize,
    old_fd: usize,
    new_fd: usize,
) -> crate::types::SyscallResult {
    start_vfs_async_request(
        alloc,
        child,
        syscall_mrs,
        VfsOp::Dup.raw(),
        &[Xv6Protocol::HostToVfs.raw(), XV6_ABI_VERSION, file as u64],
        VFS_ASYNC_DUP,
        old_fd,
        new_fd,
        0,
        0,
        [0; MAX_PATH_BYTES],
        0,
    )
}

pub(crate) fn start_vfs_fstat_request(
    alloc: &mut Allocator,
    child: &mut TaskStruct,
    syscall_mrs: &[u64; 64],
    file: usize,
    dst: u64,
) -> crate::types::SyscallResult {
    start_vfs_async_request(
        alloc,
        child,
        syscall_mrs,
        VfsOp::Fstat.raw(),
        &[Xv6Protocol::HostToVfs.raw(), XV6_ABI_VERSION, file as u64],
        VFS_ASYNC_FSTAT,
        0,
        0,
        dst,
        0,
        [0; MAX_PATH_BYTES],
        0,
    )
}

pub(crate) fn start_vfs_pipe_request(
    alloc: &mut Allocator,
    child: &mut TaskStruct,
    syscall_mrs: &[u64; 64],
    read_fd: usize,
    write_fd: usize,
    fds_ptr: u64,
) -> crate::types::SyscallResult {
    start_vfs_async_request(
        alloc,
        child,
        syscall_mrs,
        VfsOp::Pipe.raw(),
        &[Xv6Protocol::HostToVfs.raw(), XV6_ABI_VERSION],
        VFS_ASYNC_PIPE,
        read_fd,
        write_fd,
        fds_ptr,
        0,
        [0; MAX_PATH_BYTES],
        0,
    )
}

pub(crate) fn start_vfs_read_request(
    alloc: &mut Allocator,
    child: &mut TaskStruct,
    syscall_mrs: &[u64; 64],
    fd: usize,
    dst: u64,
    len: usize,
) -> crate::types::SyscallResult {
    let Some(file) = fd_file(child, fd) else {
        return crate::types::SyscallResult::Reply(-1);
    };
    let request = min(len, FS_BLOCK_SIZE);
    start_vfs_async_request(
        alloc,
        child,
        syscall_mrs,
        VfsOp::Read.raw(),
        &[
            Xv6Protocol::HostToVfs.raw(),
            XV6_ABI_VERSION,
            file as u64,
            request as u64,
        ],
        VFS_ASYNC_READ,
        fd,
        0,
        dst,
        len,
        [0; MAX_PATH_BYTES],
        0,
    )
}

pub(crate) fn start_vfs_write_request(
    alloc: &mut Allocator,
    child: &mut TaskStruct,
    syscall_mrs: &[u64; 64],
    fd: usize,
    buf: u64,
    len: usize,
) -> crate::types::SyscallResult {
    let Some(file) = fd_file(child, fd) else {
        return crate::types::SyscallResult::Reply(-1);
    };
    let request = min(len, XV6_MAX_FILE_WRITE);
    if !copy_child_to_vfs_shared_buffer(alloc, child, buf, request) {
        return crate::types::SyscallResult::Reply(-1);
    }
    fence(Ordering::SeqCst);
    start_vfs_async_request(
        alloc,
        child,
        syscall_mrs,
        VfsOp::Write.raw(),
        &[
            Xv6Protocol::HostToVfs.raw(),
            XV6_ABI_VERSION,
            file as u64,
            request as u64,
        ],
        VFS_ASYNC_WRITE,
        fd,
        0,
        buf,
        len,
        [0; MAX_PATH_BYTES],
        0,
    )
}

fn start_vfs_async_request(
    alloc: &mut Allocator,
    child: &mut TaskStruct,
    syscall_mrs: &[u64; 64],
    label: u64,
    request_mrs: &[u64],
    kind: u8,
    fd: usize,
    fd2: usize,
    user_ptr: u64,
    len: usize,
    cwd: [u8; MAX_PATH_BYTES],
    cwd_len: usize,
) -> crate::types::SyscallResult {
    let ep = VFS_SERVER_EP.load(Ordering::Relaxed);
    if ep == 0 || request_mrs.len() > 64 {
        return crate::types::SyscallResult::Reply(-1);
    }
    let Some(slot) = alloc_vfs_async_request() else {
        return crate::types::SyscallResult::Reply(-1);
    };
    let request_id = next_vfs_request_id();
    let (reply_slot, reply_mrs) = save_child_reply(alloc, syscall_mrs);
    VFS_CLIENT.set_async_request(
        slot,
        VfsAsyncRequest {
            active: true,
            request_id,
            pid: child.pid,
            reply_slot,
            reply_mrs,
            kind,
            fd,
            fd2,
            user_ptr,
            len,
            done: 0,
            cwd,
            cwd_len,
        },
    );
    let mut mrs = [0u64; 64];
    let mut i = 0usize;
    while i < request_mrs.len() {
        mrs[i] = request_mrs[i];
        i += 1;
    }
    mrs[0] = Xv6Protocol::HostToVfsAsync.raw();
    mrs[1] = request_id;
    unsafe {
        sel4_send(
            ep,
            msg_info(label, 0, 0, request_mrs.len() as u64),
            &mrs[..request_mrs.len()],
        );
    }
    child.state = PROC_VFS_ASYNC;
    crate::types::SyscallResult::Block
}

pub(crate) fn resume_vfs_waiter_async(
    alloc: &mut Allocator,
    child: &mut TaskStruct,
    state: u8,
) -> bool {
    let ep = VFS_SERVER_EP.load(Ordering::Relaxed);
    if ep == 0 || child.vfs_reply_slot == 0 || child.vfs_done >= child.vfs_len {
        return false;
    }
    let Some(file) = fd_file(child, child.vfs_fd) else {
        return false;
    };
    let remaining = child.vfs_len - child.vfs_done;
    let (label, kind, request) = if state == PROC_VFS_READ {
        (
            VfsOp::Read.raw(),
            VFS_ASYNC_READ,
            min(remaining, FS_BLOCK_SIZE),
        )
    } else if state == PROC_VFS_WRITE {
        let request = min(remaining, XV6_MAX_FILE_WRITE);
        if !copy_child_to_vfs_shared_buffer(
            alloc,
            child,
            child.vfs_buf + child.vfs_done as u64,
            request,
        ) {
            return false;
        }
        fence(Ordering::SeqCst);
        (VfsOp::Write.raw(), VFS_ASYNC_WRITE, request)
    } else {
        return false;
    };
    let Some(slot) = alloc_vfs_async_request() else {
        return false;
    };
    let request_id = next_vfs_request_id();
    VFS_CLIENT.set_async_request(
        slot,
        VfsAsyncRequest {
            active: true,
            request_id,
            pid: child.pid,
            reply_slot: child.vfs_reply_slot,
            reply_mrs: child.vfs_reply_mrs,
            kind,
            fd: child.vfs_fd,
            fd2: 0,
            user_ptr: child.vfs_buf,
            len: child.vfs_len,
            done: child.vfs_done,
            cwd: [0; MAX_PATH_BYTES],
            cwd_len: 0,
        },
    );
    unsafe {
        sel4_send(
            ep,
            msg_info(label, 0, 0, 4),
            &[
                Xv6Protocol::HostToVfsAsync.raw(),
                request_id,
                file as u64,
                request as u64,
            ],
        );
    }
    child.state = PROC_VFS_ASYNC;
    child.vfs_reply_slot = 0;
    child.vfs_reply_mrs = [0; arch::FAULT_REPLY_WORDS];
    child.vfs_fd = 0;
    child.vfs_buf = 0;
    child.vfs_len = 0;
    child.vfs_done = 0;
    true
}

pub(crate) fn complete_vfs_async_reply(
    alloc: &mut Allocator,
    procs: &mut [TaskStruct; MAX_PROCS],
    msg: &IpcMessage,
) -> Option<bool> {
    if msg_label(msg.info) != Xv6Protocol::HostToVfsAsync.raw() || msg_len(msg.info) < 5 {
        return None;
    }
    let request_id = msg.mrs[0];
    let Some(slot) = find_vfs_async_request(request_id) else {
        return Some(false);
    };
    let request = VFS_CLIENT.take_async_request(slot);
    let completion = complete_vfs_async_state(alloc, procs, &request, msg);
    if let Some(ret) = completion.ret {
        let mut reply_mrs = request.reply_mrs;
        arch::set_syscall_return_value(&mut reply_mrs, ret as u64);
        unsafe {
            sel4_send(
                request.reply_slot,
                msg_info(0, 0, 0, arch::FAULT_REPLY_WORDS as u64),
                &reply_mrs,
            );
        }
        alloc.delete_cap_slot(request.reply_slot);
    }
    Some(completion.pump_waiters)
}

pub(crate) fn has_active_vfs_async_requests() -> bool {
    VFS_CLIENT.has_active_async_requests()
}

struct VfsCompletion {
    ret: Option<i64>,
    pump_waiters: bool,
}

impl VfsCompletion {
    const fn reply(ret: i64) -> Self {
        Self {
            ret: Some(ret),
            pump_waiters: true,
        }
    }

    const fn wait() -> Self {
        Self {
            ret: None,
            pump_waiters: false,
        }
    }

    const fn progress_wait() -> Self {
        Self {
            ret: None,
            pump_waiters: true,
        }
    }
}

fn complete_vfs_async_state(
    alloc: &mut Allocator,
    procs: &mut [TaskStruct; MAX_PROCS],
    request: &VfsAsyncRequest,
    msg: &IpcMessage,
) -> VfsCompletion {
    let status = msg.mrs[1];
    let ok = status == Xv6Status::Ok.raw();
    let mut ret = if ok { 0 } else { -1 };
    for child in procs.iter_mut() {
        if child.pid != request.pid {
            continue;
        }
        if request.kind == VFS_ASYNC_READ && status == Xv6Status::WouldBlock.raw() {
            set_vfs_waiter(child, request, PROC_VFS_READ, request.done);
            return VfsCompletion::wait();
        }
        if request.kind == VFS_ASYNC_WRITE && status == Xv6Status::WouldBlock.raw() {
            set_vfs_waiter(child, request, PROC_VFS_WRITE, request.done);
            return VfsCompletion::wait();
        }
        if ok {
            match request.kind {
                VFS_ASYNC_OPEN => {
                    let file = msg.mrs[2] as usize;
                    if request.fd < MAX_FD && file < MAX_OPEN_FILES {
                        child.fds[request.fd] = file;
                        child.fd_serial[request.fd] =
                            msg.mrs[3] != Xv6FileType::Device.raw() as u64;
                        ret = request.fd as i64;
                    } else {
                        ret = -1;
                    }
                }
                VFS_ASYNC_CHDIR => {
                    child.cwd = request.cwd;
                    child.cwd_len = request.cwd_len;
                    child.cwd_inode = msg.mrs[2] as u32;
                    ret = 0;
                }
                VFS_ASYNC_CLOSE => {
                    if request.fd < MAX_FD {
                        child.fds[request.fd] = MAX_OPEN_FILES;
                        child.fd_serial[request.fd] = false;
                        ret = 0;
                    } else {
                        ret = -1;
                    }
                }
                VFS_ASYNC_DUP => {
                    if request.fd < MAX_FD && request.fd2 < MAX_FD {
                        child.fds[request.fd2] = child.fds[request.fd];
                        child.fd_serial[request.fd2] = child.fd_serial[request.fd];
                        ret = request.fd2 as i64;
                    } else {
                        ret = -1;
                    }
                }
                VFS_ASYNC_FSTAT => {
                    ret = complete_fstat(alloc, child, request.user_ptr, msg);
                }
                VFS_ASYNC_PIPE => {
                    ret =
                        complete_pipe(alloc, child, request.fd, request.fd2, request.user_ptr, msg);
                }
                VFS_ASYNC_READ => {
                    let n = msg.mrs[2] as usize;
                    let first_request = min(request.len - request.done, FS_BLOCK_SIZE);
                    if n <= first_request
                        && copy_vfs_shared_buffer_to_child(
                            alloc,
                            child,
                            request.user_ptr + request.done as u64,
                            n,
                        )
                    {
                        let total = request.done + n;
                        if n > 0
                            && total < request.len
                            && request.fd < MAX_FD
                            && child.fd_serial[request.fd]
                        {
                            set_vfs_waiter(child, request, PROC_VFS_READ, total);
                            return VfsCompletion::progress_wait();
                        }
                        ret = total as i64;
                    } else {
                        ret = -1;
                    }
                }
                VFS_ASYNC_WRITE => {
                    let n = msg.mrs[2] as usize;
                    let first_request = min(request.len - request.done, XV6_MAX_FILE_WRITE);
                    if n == 0 || n > first_request {
                        ret = if request.done == 0 {
                            -1
                        } else {
                            request.done as i64
                        };
                    } else if request.done + n < request.len {
                        set_vfs_waiter(child, request, PROC_VFS_WRITE, request.done + n);
                        return VfsCompletion::progress_wait();
                    } else {
                        ret = (request.done + n) as i64;
                    }
                }
                _ => {}
            }
        } else if request.kind == VFS_ASYNC_WRITE && status == Xv6Status::BrokenPipe.raw() {
            ret = if request.done == 0 {
                -1
            } else {
                request.done as i64
            };
        }
        if child.state == PROC_VFS_ASYNC {
            child.state = PROC_RUNNABLE;
        }
        break;
    }
    VfsCompletion::reply(ret)
}

fn set_vfs_waiter(child: &mut TaskStruct, request: &VfsAsyncRequest, state: u8, done: usize) {
    child.state = state;
    child.vfs_reply_slot = request.reply_slot;
    child.vfs_reply_mrs = request.reply_mrs;
    child.vfs_fd = request.fd;
    child.vfs_buf = request.user_ptr;
    child.vfs_len = request.len;
    child.vfs_done = done;
}

fn complete_fstat(
    alloc: &mut Allocator,
    child: &TaskStruct,
    user_ptr: u64,
    msg: &IpcMessage,
) -> i64 {
    let typ_nlink = msg.mrs[2];
    let typ = unpack_stat_type(typ_nlink);
    let ino = msg.mrs[3] as u32;
    let nlink = unpack_stat_nlink(typ_nlink);
    let size = msg.mrs[4];
    let mut st = [0u8; 24];
    write_i32(&mut st, 0, 1);
    write_u32(&mut st, 4, ino);
    write_u16(&mut st, 8, typ);
    write_u16(&mut st, 10, nlink);
    write_u64_bytes(&mut st, 16, size);
    if copy_to_child(alloc, child, user_ptr, &st) {
        0
    } else {
        -1
    }
}

fn complete_pipe(
    alloc: &mut Allocator,
    child: &mut TaskStruct,
    read_fd: usize,
    write_fd: usize,
    user_ptr: u64,
    msg: &IpcMessage,
) -> i64 {
    let read_file = msg.mrs[2] as usize;
    let write_file = msg.mrs[3] as usize;
    if read_fd >= MAX_FD
        || write_fd >= MAX_FD
        || read_file >= MAX_OPEN_FILES
        || write_file >= MAX_OPEN_FILES
    {
        return -1;
    }
    let mut out = [0u8; 8];
    write_i32(&mut out, 0, read_fd as i32);
    write_i32(&mut out, 4, write_fd as i32);
    if !copy_to_child(alloc, child, user_ptr, &out) {
        return -1;
    }
    child.fds[read_fd] = read_file;
    child.fds[write_fd] = write_file;
    child.fd_serial[read_fd] = false;
    child.fd_serial[write_fd] = false;
    0
}

fn save_child_reply(alloc: &mut Allocator, mrs: &[u64; 64]) -> (u64, arch::FaultReplyFrame) {
    let override_slot = DEFERRED_REPLY_OVERRIDE.swap(0, Ordering::Relaxed);
    if override_slot != 0 {
        return (override_slot, arch::syscall_reply_frame(mrs));
    }
    let reply_slot = alloc.alloc_slot();
    call_checked(
        ROOT_CNODE,
        LABEL_CNODE_SAVE_CALLER,
        &[],
        &[reply_slot, ROOT_CNODE_DEPTH],
    );
    (reply_slot, arch::syscall_reply_frame(mrs))
}

pub(crate) fn use_deferred_reply_slot(reply_slot: u64) {
    DEFERRED_REPLY_OVERRIDE.store(reply_slot, Ordering::Relaxed);
}

fn alloc_vfs_async_request() -> Option<usize> {
    VFS_CLIENT.alloc_async_request()
}

fn find_vfs_async_request(request_id: u64) -> Option<usize> {
    VFS_CLIENT.find_async_request(request_id)
}

fn next_vfs_request_id() -> u64 {
    loop {
        let id = NEXT_VFS_REQUEST_ID.load(Ordering::Relaxed);
        let next_id = match id.wrapping_add(1) {
            0 => 1,
            id => id,
        };
        if NEXT_VFS_REQUEST_ID
            .compare_exchange_weak(id, next_id, Ordering::Relaxed, Ordering::Relaxed)
            .is_ok()
        {
            return id;
        }
    }
}

pub(crate) fn init_vfs_process(child: &mut TaskStruct) {
    if vfs_proc_init(child) != 0 {
        warn!("xv6-host: failed to initialize vfs process");
        halt_loop();
    }
}

fn vfs_proc_init(child: &mut TaskStruct) -> i64 {
    let Some(reply) = vfs_call(
        VfsOp::ProcInit.raw(),
        &[Xv6Protocol::HostToVfs.raw(), XV6_ABI_VERSION],
    ) else {
        return -1;
    };
    if reply.mrs[0] != Xv6Status::Ok.raw() {
        return -1;
    }
    if reply.mrs[1] as usize >= MAX_OPEN_FILES
        || reply.mrs[2] as usize >= MAX_OPEN_FILES
        || reply.mrs[3] as usize >= MAX_OPEN_FILES
    {
        return -1;
    }
    child.fds = [MAX_OPEN_FILES; MAX_FD];
    child.fd_serial = [false; MAX_FD];
    child.fds[0] = reply.mrs[1] as usize;
    child.fds[1] = reply.mrs[2] as usize;
    child.fds[2] = reply.mrs[3] as usize;
    child.cwd = [0; MAX_PATH_BYTES];
    child.cwd[0] = b'/';
    child.cwd_len = 1;
    child.cwd_inode = ROOT_INO;
    0
}

pub(crate) fn fd_file(child: &TaskStruct, fd: usize) -> Option<usize> {
    if fd >= MAX_FD || child.fds[fd] >= MAX_OPEN_FILES {
        None
    } else {
        Some(child.fds[fd])
    }
}

fn child_path(child: &TaskStruct) -> &[u8] {
    &child.cwd[..child.cwd_len]
}

pub(crate) fn resolve_child_path(
    child: &TaskStruct,
    path: &[u8],
    out: &mut [u8; MAX_PATH_BYTES],
) -> Option<usize> {
    if path.is_empty() {
        return None;
    }
    out.fill(0);
    let mut out_len = 1usize;
    out[0] = b'/';
    if path[0] != b'/' {
        let cwd = child_path(child);
        if cwd.is_empty() || cwd[0] != b'/' || cwd.len() > MAX_PATH_BYTES {
            return None;
        }
        out[..cwd.len()].copy_from_slice(cwd);
        out_len = cwd.len();
    }

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
        let component = &path[start..pos];
        if component == b"." {
            continue;
        }
        if component == b".." {
            if out_len > 1 {
                while out_len > 1 && out[out_len - 1] != b'/' {
                    out_len -= 1;
                }
                if out_len > 1 {
                    out_len -= 1;
                }
                let mut i = out_len;
                while i < MAX_PATH_BYTES {
                    out[i] = 0;
                    i += 1;
                }
            }
            continue;
        }
        let need_slash = out_len > 1;
        let extra = component.len() + if need_slash { 1 } else { 0 };
        if component.is_empty() || out_len + extra > MAX_PATH_BYTES {
            return None;
        }
        if need_slash {
            out[out_len] = b'/';
            out_len += 1;
        }
        out[out_len..out_len + component.len()].copy_from_slice(component);
        out_len += component.len();
    }
    if out_len == 0 {
        out[0] = b'/';
        out_len = 1;
    }
    Some(out_len)
}

pub(crate) fn find_free_fd(child: &TaskStruct) -> Option<usize> {
    let mut fd = 0usize;
    while fd < MAX_FD {
        if child.fds[fd] >= MAX_OPEN_FILES {
            return Some(fd);
        }
        fd += 1;
    }
    None
}

pub(crate) fn find_two_free_fds(child: &TaskStruct) -> Option<(usize, usize)> {
    let first = find_free_fd(child)?;
    let mut second = first + 1;
    while second < MAX_FD {
        if child.fds[second] >= MAX_OPEN_FILES {
            return Some((first, second));
        }
        second += 1;
    }
    None
}

pub(crate) fn vfs_retain_file(file: usize) -> bool {
    let Some(reply) = vfs_call(
        VfsOp::Dup.raw(),
        &[Xv6Protocol::HostToVfs.raw(), XV6_ABI_VERSION, file as u64],
    ) else {
        return false;
    };
    reply.mrs[0] == Xv6Status::Ok.raw()
}

pub(crate) fn vfs_release_file(file: usize) -> bool {
    let Some(reply) = vfs_call(
        VfsOp::Close.raw(),
        &[Xv6Protocol::HostToVfs.raw(), XV6_ABI_VERSION, file as u64],
    ) else {
        return false;
    };
    reply.mrs[0] == Xv6Status::Ok.raw()
}

pub(crate) fn vfs_retain_cwd(inum: u32) -> bool {
    if inum == 0 {
        return true;
    }
    let Some(reply) = vfs_call(
        VfsOp::ProcFork.raw(),
        &[Xv6Protocol::HostToVfs.raw(), XV6_ABI_VERSION, inum as u64],
    ) else {
        return false;
    };
    reply.mrs[0] == Xv6Status::Ok.raw()
}

pub(crate) fn vfs_release_cwd(inum: u32) -> bool {
    if inum == 0 {
        return true;
    }
    let Some(reply) = vfs_call(
        VfsOp::ProcExit.raw(),
        &[Xv6Protocol::HostToVfs.raw(), XV6_ABI_VERSION, inum as u64],
    ) else {
        return false;
    };
    reply.mrs[0] == Xv6Status::Ok.raw()
}

pub(crate) fn release_cwd_ref(child: &mut TaskStruct) {
    if child.cwd_inode != 0 && vfs_release_cwd(child.cwd_inode) {
        child.cwd_inode = 0;
    }
}

pub(crate) fn close_fd(child: &mut TaskStruct, fd: usize) -> bool {
    let Some(file) = fd_file(child, fd) else {
        return false;
    };
    if !vfs_release_file(file) {
        return false;
    }
    child.fds[fd] = MAX_OPEN_FILES;
    child.fd_serial[fd] = false;
    true
}

pub(crate) fn close_all_fds(child: &mut TaskStruct) {
    let mut fd = 0usize;
    while fd < MAX_FD {
        if child.fds[fd] < MAX_OPEN_FILES {
            let _ = close_fd(child, fd);
        }
        fd += 1;
    }
}

pub(crate) fn retain_fd_refs(child: &TaskStruct) -> bool {
    let mut fd = 0usize;
    while fd < MAX_FD {
        if child.fds[fd] < MAX_OPEN_FILES && !vfs_retain_file(child.fds[fd]) {
            let mut undo = 0usize;
            while undo < fd {
                if child.fds[undo] < MAX_OPEN_FILES {
                    let _ = vfs_release_file(child.fds[undo]);
                }
                undo += 1;
            }
            return false;
        }
        fd += 1;
    }
    true
}

pub(crate) fn vfs_read_exec_image(child: &TaskStruct, path: &[u8]) -> Option<&'static [u8]> {
    if let Some(image) = vfs_read_exec_image_at(child, path) {
        return Some(image);
    }
    if !exec_root_fallback_allowed(path) || path.len() + 1 > MAX_PATH_BYTES {
        return None;
    }

    let mut root_path = [0u8; MAX_PATH_BYTES];
    root_path[0] = b'/';
    root_path[1..1 + path.len()].copy_from_slice(path);
    vfs_read_exec_image_at(child, &root_path[..path.len() + 1])
}

fn vfs_read_exec_image_at(child: &TaskStruct, path: &[u8]) -> Option<&'static [u8]> {
    let (handle, size) = vfs_exec_open(child, path)?;
    if size == 0 || size > MAX_FILE_BYTES {
        vfs_exec_close(handle);
        return None;
    }

    let dst = VFS_CLIENT.exec_image_buf_ptr();
    let mut done = 0usize;
    while done < size {
        let request = min(size - done, FS_BLOCK_SIZE);
        let Some(n) = vfs_exec_read(handle, done, request) else {
            vfs_exec_close(handle);
            return None;
        };
        if n == 0 {
            vfs_exec_close(handle);
            return None;
        }
        fence(Ordering::SeqCst);
        unsafe {
            core::ptr::copy_nonoverlapping(vfs_shared_buffer_ptr() as *const u8, dst.add(done), n);
        }
        done += n;
    }
    vfs_exec_close(handle);

    let image = VFS_CLIENT.exec_image(done);
    elf_image_valid(image).then_some(image)
}

fn exec_root_fallback_allowed(path: &[u8]) -> bool {
    if path.is_empty() || path[0] == b'/' {
        return false;
    }
    let mut i = 0usize;
    while i < path.len() {
        if path[i] == b'/' {
            return false;
        }
        i += 1;
    }
    true
}

fn vfs_exec_open(child: &TaskStruct, path: &[u8]) -> Option<(u32, usize)> {
    let mut resolved = [0u8; MAX_PATH_BYTES];
    let path_len = resolve_child_path(child, path, &mut resolved)?;
    let mut mrs = [0u64; 64];
    mrs[0] = Xv6Protocol::HostToVfs.raw();
    mrs[1] = XV6_ABI_VERSION;
    mrs[2] = path_len as u64;
    pack_path_words(&resolved[..path_len], &mut mrs, 3);
    let reply = vfs_call(VfsOp::ExecOpen.raw(), &mrs[..3 + path_len.div_ceil(8)])?;
    (reply.mrs[0] == Xv6Status::Ok.raw()).then_some((reply.mrs[1] as u32, reply.mrs[2] as usize))
}

fn vfs_exec_read(handle: u32, offset: usize, len: usize) -> Option<usize> {
    let request = min(len, FS_BLOCK_SIZE);
    let reply = vfs_call(
        VfsOp::ExecRead.raw(),
        &[
            Xv6Protocol::HostToVfs.raw(),
            XV6_ABI_VERSION,
            handle as u64,
            offset as u64,
            0,
            request as u64,
        ],
    )?;
    if reply.mrs[0] != Xv6Status::Ok.raw() {
        return None;
    }
    let n = reply.mrs[1] as usize;
    (n <= request).then_some(n)
}

fn vfs_exec_close(handle: u32) {
    let _ = vfs_call(
        VfsOp::ExecClose.raw(),
        &[Xv6Protocol::HostToVfs.raw(), XV6_ABI_VERSION, handle as u64],
    );
}

pub(crate) fn pack_path_words(path: &[u8], mrs: &mut [u64], start: usize) {
    let mut i = 0usize;
    while i < path.len() {
        mrs[start + i / 8] |= (path[i] as u64) << ((i % 8) * 8);
        i += 1;
    }
}

pub(crate) fn vfs_call(label: u64, mrs: &[u64]) -> Option<IpcMessage> {
    let ep = VFS_SERVER_EP.load(Ordering::Relaxed);
    if ep == 0 {
        return None;
    }
    let info = msg_info(label, 0, 0, mrs.len() as u64);
    let mut retries = 0usize;
    loop {
        let reply = unsafe { sel4_call(ep, info, mrs) };
        if msg_label(reply.info) != 0 {
            return None;
        }
        if reply.mrs[0] != Xv6Status::Busy.raw() {
            return Some(reply);
        }
        retries += 1;
        if retries >= FS_BUSY_RETRY_LIMIT {
            warn!("xv6-host: vfs busy retry exhausted op={}", label);
            return None;
        }
        unsafe {
            sel4_yield();
        }
    }
}

fn copy_child_to_vfs_shared_buffer(
    alloc: &mut Allocator,
    child: &TaskStruct,
    src: u64,
    len: usize,
) -> bool {
    let len = min(len, XV6_MAX_FILE_WRITE);
    let dst = unsafe { core::slice::from_raw_parts_mut(vfs_shared_buffer_ptr(), len) };
    copy_from_child(alloc, child, src, dst)
}

fn copy_vfs_shared_buffer_to_child(
    alloc: &mut Allocator,
    child: &TaskStruct,
    dst: u64,
    len: usize,
) -> bool {
    let len = min(len, XV6_MAX_FILE_WRITE);
    let src = unsafe { core::slice::from_raw_parts(vfs_shared_buffer_ptr() as *const u8, len) };
    copy_to_child(alloc, child, dst, src)
}

fn vfs_shared_buffer_ptr() -> *mut u8 {
    XV6_DISK_SHARED_BUFFER_VADDR as *mut u8
}

pub(crate) fn final_component_is_dot_or_dotdot(path: &[u8]) -> bool {
    let mut end = path.len();
    while end > 0 && path[end - 1] == b'/' {
        end -= 1;
    }
    if end == 0 {
        return false;
    }
    let mut start = end;
    while start > 0 && path[start - 1] != b'/' {
        start -= 1;
    }
    let component = &path[start..end];
    component == b"." || component == b".."
}

pub(crate) fn basename(path: &[u8]) -> &[u8] {
    let mut start = 0;
    for (i, b) in path.iter().enumerate() {
        if *b == b'/' {
            start = i + 1;
        }
    }
    &path[start..]
}
