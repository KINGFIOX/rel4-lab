use core::cell::UnsafeCell;
use core::cmp::min;
use core::sync::atomic::{AtomicU64, Ordering, fence};

use sel4_user::rt::{AsyncLock, WaitCell};
use sel4_user::{IpcMessage, msg_info, msg_label, msg_len, sel4_call, sel4_send, sel4_yield};

use crate::allocator::Allocator;
use crate::child::{copy_from_child, copy_to_child, elf_image_valid};
use crate::consts::*;
use crate::host::{find_proc, with_host};
use crate::types::{SyscallResult, TaskStruct};
use crate::util::{halt_loop, warn, write_i32, write_u32, write_u64_bytes};

static VFS_SERVER_EP: AtomicU64 = AtomicU64::new(0);
static NEXT_VFS_REQUEST_ID: AtomicU64 = AtomicU64::new(1);
static VFS_GATE: AsyncLock = AsyncLock::new();
static VFS_REPLY: WaitCell<IpcMessage> = WaitCell::new();

const FS_BUSY_RETRY_LIMIT: usize = 4096;

pub(crate) fn init_vfs_client(vfs_ep: u64) {
    VFS_SERVER_EP.store(vfs_ep, Ordering::Relaxed);
}

struct VfsClientState {
    exec_image_buf: [u8; MAX_FILE_BYTES],
}

impl VfsClientState {
    const fn new() -> Self {
        Self {
            exec_image_buf: [0; MAX_FILE_BYTES],
        }
    }
}

struct VfsClient {
    state: UnsafeCell<VfsClientState>,
}

unsafe impl Sync for VfsClient {}

impl VfsClient {
    const fn new() -> Self {
        Self {
            state: UnsafeCell::new(VfsClientState::new()),
        }
    }

    fn exec_image_buf_ptr(&self) -> *mut u8 {
        unsafe { (&mut *self.state.get()).exec_image_buf.as_mut_ptr() }
    }

    fn exec_image(&self, len: usize) -> &'static [u8] {
        unsafe { core::slice::from_raw_parts((&*self.state.get()).exec_image_buf.as_ptr(), len) }
    }
}

static VFS_CLIENT: VfsClient = VfsClient::new();

pub(crate) async fn acquire_vfs() -> sel4_user::rt::AsyncLockGuard<'static> {
    VFS_GATE.lock().await
}

pub(crate) async fn vfs_rpc(label: u64, request_mrs: &[u64]) -> Option<IpcMessage> {
    let _permit = VFS_GATE.lock().await;
    vfs_rpc_locked(label, request_mrs).await
}

pub(crate) async fn vfs_rpc_locked(label: u64, request_mrs: &[u64]) -> Option<IpcMessage> {
    let ep = VFS_SERVER_EP.load(Ordering::Relaxed);
    if ep == 0 || request_mrs.len() > 64 || request_mrs.len() < 2 {
        return None;
    }
    let request_id = next_vfs_request_id();
    let mut mrs = [0u64; 64];
    mrs[..request_mrs.len()].copy_from_slice(request_mrs);
    mrs[0] = IpcProtocol::HostToVfsAsync.raw();
    mrs[1] = request_id;
    VFS_REPLY.reset();
    unsafe {
        sel4_send(
            ep,
            msg_info(label, 0, 0, request_mrs.len() as u64),
            &mrs[..request_mrs.len()],
        );
    }
    Some(VFS_REPLY.wait().await)
}

pub(crate) fn complete_vfs_async_reply(msg: &IpcMessage) -> bool {
    if msg_label(msg.info) != IpcProtocol::HostToVfsAsync.raw() || msg_len(msg.info) < 5 {
        return false;
    }
    VFS_REPLY.complete(*msg);
    true
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

pub(crate) async fn vfs_status_request(label: u64, request_mrs: &[u64]) -> SyscallResult {
    let Some(reply) = vfs_rpc(label, request_mrs).await else {
        return SyscallResult::err(EIO);
    };
    status_result(&reply)
}

pub(crate) async fn vfs_open_request(
    pid: u64,
    fd: usize,
    label: u64,
    request_mrs: &[u64],
) -> SyscallResult {
    let Some(reply) = vfs_rpc(label, request_mrs).await else {
        return SyscallResult::err(EIO);
    };
    if reply.mrs[1] != IpcStatus::Ok.raw() {
        return SyscallResult::err(reply.mrs[1] as i32);
    }
    with_host(|_, procs| {
        let Some(idx) = find_proc(procs, pid) else {
            return SyscallResult::err(ESRCH);
        };
        let file = reply.mrs[2] as usize;
        if fd < MAX_FD && file < MAX_OPEN_FILES {
            procs[idx].fds[fd] = file;
            procs[idx].fd_serial[fd] = reply.mrs[3] != FileKind::Device.raw() as u64;
            SyscallResult::Reply(fd as i64)
        } else {
            SyscallResult::err(EMFILE)
        }
    })
}

pub(crate) async fn vfs_chdir_request(
    pid: u64,
    cwd: [u8; MAX_PATH_BYTES],
    cwd_len: usize,
    label: u64,
    request_mrs: &[u64],
) -> SyscallResult {
    let Some(reply) = vfs_rpc(label, request_mrs).await else {
        return SyscallResult::err(EIO);
    };
    if reply.mrs[1] != IpcStatus::Ok.raw() {
        return SyscallResult::err(reply.mrs[1] as i32);
    }
    with_host(|_, procs| {
        let Some(idx) = find_proc(procs, pid) else {
            return SyscallResult::err(ESRCH);
        };
        procs[idx].cwd = cwd;
        procs[idx].cwd_len = cwd_len;
        procs[idx].cwd_inode = reply.mrs[2] as u32;
        SyscallResult::Reply(0)
    })
}

pub(crate) async fn vfs_close_request(pid: u64, file: usize, fd: usize) -> SyscallResult {
    let Some(reply) = vfs_rpc(
        VfsOp::Close.raw(),
        &[IpcProtocol::HostToVfs.raw(), LINUX_ABI_VERSION, file as u64],
    )
    .await
    else {
        return SyscallResult::err(EIO);
    };
    if reply.mrs[1] != IpcStatus::Ok.raw() {
        return SyscallResult::err(reply.mrs[1] as i32);
    }
    with_host(|_, procs| {
        let Some(idx) = find_proc(procs, pid) else {
            return SyscallResult::err(ESRCH);
        };
        if fd < MAX_FD {
            procs[idx].fds[fd] = MAX_OPEN_FILES;
            procs[idx].fd_serial[fd] = false;
            SyscallResult::Reply(0)
        } else {
            SyscallResult::err(EBADF)
        }
    })
}

pub(crate) async fn vfs_dup_request(
    pid: u64,
    file: usize,
    old_fd: usize,
    new_fd: usize,
) -> SyscallResult {
    let Some(reply) = vfs_rpc(
        VfsOp::Dup.raw(),
        &[IpcProtocol::HostToVfs.raw(), LINUX_ABI_VERSION, file as u64],
    )
    .await
    else {
        return SyscallResult::err(EIO);
    };
    if reply.mrs[1] != IpcStatus::Ok.raw() {
        return SyscallResult::err(reply.mrs[1] as i32);
    }
    with_host(|_, procs| {
        let Some(idx) = find_proc(procs, pid) else {
            return SyscallResult::err(ESRCH);
        };
        if old_fd < MAX_FD && new_fd < MAX_FD {
            procs[idx].fds[new_fd] = procs[idx].fds[old_fd];
            procs[idx].fd_serial[new_fd] = procs[idx].fd_serial[old_fd];
            SyscallResult::Reply(new_fd as i64)
        } else {
            SyscallResult::err(EBADF)
        }
    })
}

pub(crate) async fn vfs_fstat_request(pid: u64, file: usize, dst: u64) -> SyscallResult {
    let Some(reply) = vfs_rpc(
        VfsOp::Fstat.raw(),
        &[IpcProtocol::HostToVfs.raw(), LINUX_ABI_VERSION, file as u64],
    )
    .await
    else {
        return SyscallResult::err(EIO);
    };
    if reply.mrs[1] != IpcStatus::Ok.raw() {
        return SyscallResult::err(reply.mrs[1] as i32);
    }
    with_host(|alloc, procs| {
        let Some(idx) = find_proc(procs, pid) else {
            return SyscallResult::err(ESRCH);
        };
        SyscallResult::Reply(complete_fstat(alloc, &procs[idx], dst, &reply))
    })
}

pub(crate) async fn vfs_lseek_request(file: usize, offset: i64, whence: u64) -> SyscallResult {
    let Some(reply) = vfs_rpc(
        VfsOp::Lseek.raw(),
        &[
            IpcProtocol::HostToVfs.raw(),
            LINUX_ABI_VERSION,
            file as u64,
            offset as u64,
            whence,
        ],
    )
    .await
    else {
        return SyscallResult::err(EIO);
    };
    if reply.mrs[1] != IpcStatus::Ok.raw() {
        return SyscallResult::err(reply.mrs[1] as i32);
    }
    SyscallResult::Reply(reply.mrs[2] as i64)
}

pub(crate) async fn vfs_pipe_request(
    pid: u64,
    read_fd: usize,
    write_fd: usize,
    fds_ptr: u64,
) -> SyscallResult {
    let Some(reply) = vfs_rpc(
        VfsOp::Pipe.raw(),
        &[IpcProtocol::HostToVfs.raw(), LINUX_ABI_VERSION],
    )
    .await
    else {
        return SyscallResult::err(EIO);
    };
    if reply.mrs[1] != IpcStatus::Ok.raw() {
        return SyscallResult::err(reply.mrs[1] as i32);
    }
    with_host(|alloc, procs| {
        let Some(idx) = find_proc(procs, pid) else {
            return SyscallResult::err(ESRCH);
        };
        SyscallResult::Reply(complete_pipe(
            alloc,
            &mut procs[idx],
            read_fd,
            write_fd,
            fds_ptr,
            &reply,
        ))
    })
}

pub(crate) async fn vfs_read_request(pid: u64, fd: usize, dst: u64, len: usize) -> SyscallResult {
    let mut done = 0usize;
    loop {
        let _permit = acquire_vfs().await;
        let (file, serial, request) = match with_host(|_, procs| {
            let idx = find_proc(procs, pid)?;
            let file = fd_file(&procs[idx], fd)?;
            let remaining = len.saturating_sub(done);
            Some((
                file,
                procs[idx].fd_serial[fd],
                min(remaining, PAGE_SIZE as usize),
            ))
        }) {
            Some(values) => values,
            None => return SyscallResult::err(EBADF),
        };
        if request == 0 {
            return SyscallResult::Reply(done as i64);
        }
        let Some(reply) = vfs_rpc_locked(
            VfsOp::Read.raw(),
            &[
                IpcProtocol::HostToVfs.raw(),
                LINUX_ABI_VERSION,
                file as u64,
                request as u64,
            ],
        )
        .await
        else {
            return SyscallResult::err(EIO);
        };
        let status = reply.mrs[1];
        if status == IpcStatus::WouldBlock.raw() {
            drop(_permit);
            sel4_user::rt::sleep_until(sel4_user::rt::now().saturating_add(1)).await;
            continue;
        }
        if status != IpcStatus::Ok.raw() {
            return if done == 0 {
                SyscallResult::err(status as i32)
            } else {
                SyscallResult::Reply(done as i64)
            };
        }
        let n = reply.mrs[2] as usize;
        if n > request {
            return SyscallResult::err(EIO);
        }
        let copied = with_host(|alloc, procs| {
            let Some(idx) = find_proc(procs, pid) else {
                return false;
            };
            copy_vfs_shared_buffer_to_child(alloc, &procs[idx], dst + done as u64, n)
        });
        if !copied {
            return SyscallResult::err(EFAULT);
        }
        done += n;
        if n == 0 || done >= len || !serial {
            return SyscallResult::Reply(done as i64);
        }
    }
}

pub(crate) async fn vfs_write_request(pid: u64, fd: usize, buf: u64, len: usize) -> SyscallResult {
    let mut done = 0usize;
    loop {
        let _permit = acquire_vfs().await;
        let (file, request) = match with_host(|alloc, procs| {
            let idx = find_proc(procs, pid)?;
            let file = fd_file(&procs[idx], fd)?;
            let remaining = len.saturating_sub(done);
            let request = min(remaining, MAX_IO_BYTES);
            if request == 0 {
                return Some((file, 0usize));
            }
            if !copy_child_to_vfs_shared_buffer(alloc, &procs[idx], buf + done as u64, request) {
                return None;
            }
            fence(Ordering::SeqCst);
            Some((file, request))
        }) {
            Some(values) => values,
            None => {
                return if done == 0 {
                    SyscallResult::err(EFAULT)
                } else {
                    SyscallResult::Reply(done as i64)
                };
            }
        };
        if request == 0 {
            return SyscallResult::Reply(done as i64);
        }
        let Some(reply) = vfs_rpc_locked(
            VfsOp::Write.raw(),
            &[
                IpcProtocol::HostToVfs.raw(),
                LINUX_ABI_VERSION,
                file as u64,
                request as u64,
            ],
        )
        .await
        else {
            return SyscallResult::err(EIO);
        };
        let status = reply.mrs[1];
        if status == IpcStatus::WouldBlock.raw() {
            drop(_permit);
            sel4_user::rt::sleep_until(sel4_user::rt::now().saturating_add(1)).await;
            continue;
        }
        if status == IpcStatus::BrokenPipe.raw() {
            return if done == 0 {
                SyscallResult::Reply(-1)
            } else {
                SyscallResult::Reply(done as i64)
            };
        }
        if status != IpcStatus::Ok.raw() {
            return if done == 0 {
                SyscallResult::err(status as i32)
            } else {
                SyscallResult::Reply(done as i64)
            };
        }
        let n = reply.mrs[2] as usize;
        if n == 0 || n > request {
            return if done == 0 {
                SyscallResult::Reply(-1)
            } else {
                SyscallResult::Reply(done as i64)
            };
        }
        done += n;
        if done >= len {
            return SyscallResult::Reply(done as i64);
        }
    }
}

fn status_result(reply: &IpcMessage) -> SyscallResult {
    if reply.mrs[1] == IpcStatus::Ok.raw() {
        SyscallResult::Reply(0)
    } else {
        SyscallResult::err(reply.mrs[1] as i32)
    }
}

fn complete_fstat(
    alloc: &mut Allocator,
    child: &TaskStruct,
    user_ptr: u64,
    msg: &IpcMessage,
) -> i64 {
    let kind_nlink = msg.mrs[2];
    let kind = unpack_stat_kind(kind_nlink);
    let ino = msg.mrs[3];
    let nlink = unpack_stat_nlink(kind_nlink);
    let size = msg.mrs[4];
    let mode = match FileKind::from_raw(kind) {
        Some(FileKind::Directory) => S_IFDIR | 0o755,
        Some(FileKind::Device) => S_IFCHR | 0o666,
        _ => S_IFREG | 0o644,
    };
    let mut st = [0u8; LINUX_STAT_SIZE];
    write_u64_bytes(&mut st, 0, 1);
    write_u64_bytes(&mut st, 8, ino);
    write_u32(&mut st, 16, mode);
    write_u32(&mut st, 20, nlink as u32);
    write_u64_bytes(&mut st, 48, size);
    write_u32(&mut st, 56, PAGE_SIZE as u32);
    if copy_to_child(alloc, child, user_ptr, &st) {
        0
    } else {
        -(EFAULT as i64)
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

pub(crate) fn init_vfs_process(child: &mut TaskStruct) {
    if vfs_proc_init(child) != 0 {
        warn!("linux-compat: failed to initialize vfs process");
        halt_loop();
    }
}

fn vfs_proc_init(child: &mut TaskStruct) -> i64 {
    let Some(reply) = vfs_call(
        VfsOp::ProcInit.raw(),
        &[IpcProtocol::HostToVfs.raw(), LINUX_ABI_VERSION],
    ) else {
        return -1;
    };
    if reply.mrs[0] != IpcStatus::Ok.raw() {
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
        &[IpcProtocol::HostToVfs.raw(), LINUX_ABI_VERSION, file as u64],
    ) else {
        return false;
    };
    reply.mrs[0] == IpcStatus::Ok.raw()
}

pub(crate) fn vfs_release_file(file: usize) -> bool {
    let Some(reply) = vfs_call(
        VfsOp::Close.raw(),
        &[IpcProtocol::HostToVfs.raw(), LINUX_ABI_VERSION, file as u64],
    ) else {
        return false;
    };
    reply.mrs[0] == IpcStatus::Ok.raw()
}

pub(crate) fn vfs_retain_cwd(inum: u32) -> bool {
    if inum == 0 {
        return true;
    }
    let Some(reply) = vfs_call(
        VfsOp::ProcFork.raw(),
        &[IpcProtocol::HostToVfs.raw(), LINUX_ABI_VERSION, inum as u64],
    ) else {
        return false;
    };
    reply.mrs[0] == IpcStatus::Ok.raw()
}

pub(crate) fn vfs_release_cwd(inum: u32) -> bool {
    if inum == 0 {
        return true;
    }
    let Some(reply) = vfs_call(
        VfsOp::ProcExit.raw(),
        &[IpcProtocol::HostToVfs.raw(), LINUX_ABI_VERSION, inum as u64],
    ) else {
        return false;
    };
    reply.mrs[0] == IpcStatus::Ok.raw()
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
        let request = min(size - done, PAGE_SIZE as usize);
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
    mrs[0] = IpcProtocol::HostToVfs.raw();
    mrs[1] = LINUX_ABI_VERSION;
    mrs[2] = path_len as u64;
    pack_path_words(&resolved[..path_len], &mut mrs, 3);
    let reply = vfs_call(VfsOp::ExecOpen.raw(), &mrs[..3 + path_len.div_ceil(8)])?;
    (reply.mrs[0] == IpcStatus::Ok.raw()).then_some((reply.mrs[1] as u32, reply.mrs[2] as usize))
}

fn vfs_exec_read(handle: u32, offset: usize, len: usize) -> Option<usize> {
    let request = min(len, PAGE_SIZE as usize);
    let reply = vfs_call(
        VfsOp::ExecRead.raw(),
        &[
            IpcProtocol::HostToVfs.raw(),
            LINUX_ABI_VERSION,
            handle as u64,
            offset as u64,
            0,
            request as u64,
        ],
    )?;
    if reply.mrs[0] != IpcStatus::Ok.raw() {
        return None;
    }
    let n = reply.mrs[1] as usize;
    (n <= request).then_some(n)
}

fn vfs_exec_close(handle: u32) {
    let _ = vfs_call(
        VfsOp::ExecClose.raw(),
        &[
            IpcProtocol::HostToVfs.raw(),
            LINUX_ABI_VERSION,
            handle as u64,
        ],
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
        if reply.mrs[0] != IpcStatus::Busy.raw() {
            return Some(reply);
        }
        retries += 1;
        if retries >= FS_BUSY_RETRY_LIMIT {
            warn!("linux-compat: vfs busy retry exhausted op={}", label);
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
    let len = min(len, MAX_IO_BYTES);
    let dst = unsafe { core::slice::from_raw_parts_mut(vfs_shared_buffer_ptr(), len) };
    copy_from_child(alloc, child, src, dst)
}

fn copy_vfs_shared_buffer_to_child(
    alloc: &mut Allocator,
    child: &TaskStruct,
    dst: u64,
    len: usize,
) -> bool {
    let len = min(len, MAX_IO_BYTES);
    let src = unsafe { core::slice::from_raw_parts(vfs_shared_buffer_ptr() as *const u8, len) };
    copy_to_child(alloc, child, dst, src)
}

fn vfs_shared_buffer_ptr() -> *mut u8 {
    SHARED_BUFFER_VADDR as *mut u8
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
