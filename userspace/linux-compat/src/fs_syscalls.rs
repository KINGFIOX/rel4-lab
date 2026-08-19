use crate::allocator::Allocator;
use crate::child::copy_cstr_from_child;
use crate::consts::*;
use crate::types::{SyscallResult, TaskStruct};
use crate::vfs::{
    fd_file, find_free_fd, find_two_free_fds, pack_path_words, resolve_child_path,
    start_vfs_chdir_request, start_vfs_close_request, start_vfs_dup_request,
    start_vfs_fstat_request, start_vfs_lseek_request, start_vfs_open_request,
    start_vfs_pipe_request, start_vfs_status_request,
};

pub(crate) fn sys_openat(
    alloc: &mut Allocator,
    child: &mut TaskStruct,
    dirfd: i32,
    path_ptr: u64,
    flags: u32,
    _mode: u32,
    syscall_mrs: &[u64; 64],
) -> SyscallResult {
    let mut path = [0u8; 128];
    let Some(len) = copy_cstr_from_child(alloc, child, path_ptr, &mut path) else {
        return SyscallResult::err(EFAULT);
    };
    if len == 0 {
        return SyscallResult::err(ENOENT);
    }
    if dirfd != AT_FDCWD && path[0] != b'/' && fd_file(child, dirfd as usize).is_none() {
        return SyscallResult::err(EBADF);
    }
    let mut resolved = [0u8; MAX_PATH_BYTES];
    let Some(path_len) = resolve_child_path(child, &path[..len], &mut resolved) else {
        return SyscallResult::err(ENAMETOOLONG);
    };
    let Some(fd) = find_free_fd(child) else {
        return SyscallResult::err(EMFILE);
    };
    let mut mrs = [0u64; 64];
    mrs[0] = IpcProtocol::HostToVfs.raw();
    mrs[1] = LINUX_ABI_VERSION;
    mrs[2] = flags as u64;
    mrs[3] = path_len as u64;
    pack_path_words(&resolved[..path_len], &mut mrs, 4);
    start_vfs_open_request(
        alloc,
        child,
        syscall_mrs,
        VfsOp::Open.raw(),
        &mrs[..4 + path_len.div_ceil(8)],
        fd,
    )
}

pub(crate) fn sys_close(
    alloc: &mut Allocator,
    child: &mut TaskStruct,
    fd: usize,
    syscall_mrs: &[u64; 64],
) -> SyscallResult {
    let Some(file) = fd_file(child, fd) else {
        return SyscallResult::err(EBADF);
    };
    start_vfs_close_request(alloc, child, syscall_mrs, file, fd)
}

pub(crate) fn sys_dup(
    alloc: &mut Allocator,
    child: &mut TaskStruct,
    fd: usize,
    syscall_mrs: &[u64; 64],
) -> SyscallResult {
    let Some(file) = fd_file(child, fd) else {
        return SyscallResult::err(EBADF);
    };
    let Some(new_fd) = find_free_fd(child) else {
        return SyscallResult::err(EMFILE);
    };
    start_vfs_dup_request(alloc, child, syscall_mrs, file, fd, new_fd)
}

pub(crate) fn sys_dup3(
    alloc: &mut Allocator,
    child: &mut TaskStruct,
    oldfd: usize,
    newfd: usize,
    _flags: u32,
    syscall_mrs: &[u64; 64],
) -> SyscallResult {
    if oldfd == newfd {
        return SyscallResult::err(EINVAL);
    }
    let Some(file) = fd_file(child, oldfd) else {
        return SyscallResult::err(EBADF);
    };
    if newfd >= MAX_FD {
        return SyscallResult::err(EBADF);
    }
    if child.fds[newfd] < MAX_OPEN_FILES {
        let _ = crate::vfs::close_fd(child, newfd);
    }
    start_vfs_dup_request(alloc, child, syscall_mrs, file, oldfd, newfd)
}

pub(crate) fn sys_fstat(
    alloc: &mut Allocator,
    child: &mut TaskStruct,
    fd: usize,
    dst: u64,
    syscall_mrs: &[u64; 64],
) -> SyscallResult {
    let Some(file) = fd_file(child, fd) else {
        return SyscallResult::err(EBADF);
    };
    start_vfs_fstat_request(alloc, child, syscall_mrs, file, dst)
}

pub(crate) fn sys_chdir(
    alloc: &mut Allocator,
    child: &mut TaskStruct,
    path_ptr: u64,
    syscall_mrs: &[u64; 64],
) -> SyscallResult {
    let mut path = [0u8; 128];
    let Some(len) = copy_cstr_from_child(alloc, child, path_ptr, &mut path) else {
        return SyscallResult::err(EFAULT);
    };
    let mut resolved = [0u8; MAX_PATH_BYTES];
    let Some(path_len) = resolve_child_path(child, &path[..len], &mut resolved) else {
        return SyscallResult::err(ENAMETOOLONG);
    };
    let mut mrs = [0u64; 64];
    mrs[0] = IpcProtocol::HostToVfs.raw();
    mrs[1] = LINUX_ABI_VERSION;
    mrs[2] = child.cwd_inode as u64;
    mrs[3] = path_len as u64;
    pack_path_words(&resolved[..path_len], &mut mrs, 4);
    start_vfs_chdir_request(
        alloc,
        child,
        syscall_mrs,
        VfsOp::Chdir.raw(),
        &mrs[..4 + path_len.div_ceil(8)],
        resolved,
        path_len,
    )
}

pub(crate) fn sys_getcwd(
    alloc: &mut Allocator,
    child: &mut TaskStruct,
    buf: u64,
    size: u64,
    _syscall_mrs: &[u64; 64],
) -> SyscallResult {
    if buf == 0 {
        return SyscallResult::err(EFAULT);
    }
    if size == 0 || size < (child.cwd_len as u64 + 1) {
        return SyscallResult::err(ERANGE);
    }
    let mut out = [0u8; MAX_PATH_BYTES + 1];
    out[..child.cwd_len].copy_from_slice(&child.cwd[..child.cwd_len]);
    if !crate::child::copy_to_child(alloc, child, buf, &out[..child.cwd_len + 1]) {
        return SyscallResult::err(EFAULT);
    }
    SyscallResult::Reply(buf as i64)
}

pub(crate) fn sys_pipe2(
    alloc: &mut Allocator,
    child: &mut TaskStruct,
    fds_ptr: u64,
    _flags: u32,
    syscall_mrs: &[u64; 64],
) -> SyscallResult {
    let Some((read_fd, write_fd)) = find_two_free_fds(child) else {
        return SyscallResult::err(EMFILE);
    };
    start_vfs_pipe_request(alloc, child, syscall_mrs, read_fd, write_fd, fds_ptr)
}

pub(crate) fn sys_unlinkat(
    alloc: &mut Allocator,
    child: &mut TaskStruct,
    dirfd: i32,
    path_ptr: u64,
    flags: u32,
    syscall_mrs: &[u64; 64],
) -> SyscallResult {
    let mut path = [0u8; 128];
    let Some(len) = copy_cstr_from_child(alloc, child, path_ptr, &mut path) else {
        return SyscallResult::err(EFAULT);
    };
    if dirfd != AT_FDCWD && path.first() != Some(&b'/') && fd_file(child, dirfd as usize).is_none()
    {
        return SyscallResult::err(EBADF);
    }
    let mut resolved = [0u8; MAX_PATH_BYTES];
    let Some(path_len) = resolve_child_path(child, &path[..len], &mut resolved) else {
        return SyscallResult::err(ENAMETOOLONG);
    };
    let mut mrs = [0u64; 64];
    mrs[0] = IpcProtocol::HostToVfs.raw();
    mrs[1] = LINUX_ABI_VERSION;
    mrs[2] = path_len as u64;
    mrs[3] = flags as u64;
    pack_path_words(&resolved[..path_len], &mut mrs, 4);
    start_vfs_status_request(
        alloc,
        child,
        syscall_mrs,
        VfsOp::Unlink.raw(),
        &mrs[..4 + path_len.div_ceil(8)],
    )
}

pub(crate) fn sys_mkdirat(
    alloc: &mut Allocator,
    child: &mut TaskStruct,
    dirfd: i32,
    path_ptr: u64,
    _mode: u32,
    syscall_mrs: &[u64; 64],
) -> SyscallResult {
    let mut path = [0u8; 128];
    let Some(len) = copy_cstr_from_child(alloc, child, path_ptr, &mut path) else {
        return SyscallResult::err(EFAULT);
    };
    if dirfd != AT_FDCWD && path.first() != Some(&b'/') && fd_file(child, dirfd as usize).is_none()
    {
        return SyscallResult::err(EBADF);
    }
    let mut resolved = [0u8; MAX_PATH_BYTES];
    let Some(path_len) = resolve_child_path(child, &path[..len], &mut resolved) else {
        return SyscallResult::err(ENAMETOOLONG);
    };
    let mut mrs = [0u64; 64];
    mrs[0] = IpcProtocol::HostToVfs.raw();
    mrs[1] = LINUX_ABI_VERSION;
    mrs[2] = path_len as u64;
    pack_path_words(&resolved[..path_len], &mut mrs, 3);
    start_vfs_status_request(
        alloc,
        child,
        syscall_mrs,
        VfsOp::Mkdir.raw(),
        &mrs[..3 + path_len.div_ceil(8)],
    )
}

pub(crate) fn sys_lseek(
    alloc: &mut Allocator,
    child: &mut TaskStruct,
    fd: usize,
    offset: i64,
    whence: u64,
    syscall_mrs: &[u64; 64],
) -> SyscallResult {
    let Some(file) = fd_file(child, fd) else {
        return SyscallResult::err(EBADF);
    };
    start_vfs_lseek_request(alloc, child, syscall_mrs, file, offset, whence)
}
