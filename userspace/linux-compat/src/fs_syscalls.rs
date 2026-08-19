use crate::consts::*;
use crate::host::{find_proc, with_host};
use crate::types::SyscallResult;
use crate::vfs::{
    fd_file, find_free_fd, find_two_free_fds, pack_path_words, resolve_child_path,
    vfs_chdir_request, vfs_close_request, vfs_dup_request, vfs_fstat_request, vfs_lseek_request,
    vfs_open_request, vfs_pipe_request, vfs_status_request,
};

pub(crate) async fn sys_openat(
    pid: u64,
    dirfd: i32,
    path_ptr: u64,
    flags: u32,
    _mode: u32,
) -> SyscallResult {
    let prepared = with_host(|alloc, procs| {
        let Some(idx) = find_proc(procs, pid) else {
            return Err(SyscallResult::err(ESRCH));
        };
        let child = &mut procs[idx];
        let mut path = [0u8; 128];
        let Some(len) = crate::child::copy_cstr_from_child(alloc, child, path_ptr, &mut path)
        else {
            return Err(SyscallResult::err(EFAULT));
        };
        if len == 0 {
            return Err(SyscallResult::err(ENOENT));
        }
        if dirfd != AT_FDCWD && path[0] != b'/' && fd_file(child, dirfd as usize).is_none() {
            return Err(SyscallResult::err(EBADF));
        }
        let mut resolved = [0u8; MAX_PATH_BYTES];
        let Some(path_len) = resolve_child_path(child, &path[..len], &mut resolved) else {
            return Err(SyscallResult::err(ENAMETOOLONG));
        };
        let Some(fd) = find_free_fd(child) else {
            return Err(SyscallResult::err(EMFILE));
        };
        let mut mrs = [0u64; 64];
        mrs[0] = IpcProtocol::HostToVfs.raw();
        mrs[1] = LINUX_ABI_VERSION;
        mrs[2] = flags as u64;
        mrs[3] = path_len as u64;
        pack_path_words(&resolved[..path_len], &mut mrs, 4);
        Ok((fd, mrs, 4 + path_len.div_ceil(8)))
    });
    let (fd, mrs, mr_len) = match prepared {
        Ok(values) => values,
        Err(err) => return err,
    };
    vfs_open_request(pid, fd, VfsOp::Open.raw(), &mrs[..mr_len]).await
}

pub(crate) async fn sys_close(pid: u64, fd: usize) -> SyscallResult {
    let Some(file) = with_host(|_, procs| {
        let idx = find_proc(procs, pid)?;
        fd_file(&procs[idx], fd)
    }) else {
        return SyscallResult::err(EBADF);
    };
    vfs_close_request(pid, file, fd).await
}

pub(crate) async fn sys_dup(pid: u64, fd: usize) -> SyscallResult {
    let prepared = with_host(|_, procs| {
        let Some(idx) = find_proc(procs, pid) else {
            return Err(SyscallResult::err(ESRCH));
        };
        let Some(file) = fd_file(&procs[idx], fd) else {
            return Err(SyscallResult::err(EBADF));
        };
        let Some(new_fd) = find_free_fd(&procs[idx]) else {
            return Err(SyscallResult::err(EMFILE));
        };
        Ok((file, new_fd))
    });
    let (file, new_fd) = match prepared {
        Ok(values) => values,
        Err(err) => return err,
    };
    vfs_dup_request(pid, file, fd, new_fd).await
}

pub(crate) async fn sys_dup3(pid: u64, oldfd: usize, newfd: usize, _flags: u32) -> SyscallResult {
    if oldfd == newfd {
        return SyscallResult::err(EINVAL);
    }
    let _permit = crate::vfs::acquire_vfs().await;
    let Some(file) = with_host(|_, procs| {
        let idx = find_proc(procs, pid)?;
        if newfd >= MAX_FD {
            return None;
        }
        if procs[idx].fds[newfd] < MAX_OPEN_FILES {
            let _ = crate::vfs::close_fd(&mut procs[idx], newfd);
        }
        fd_file(&procs[idx], oldfd)
    }) else {
        return SyscallResult::err(EBADF);
    };
    drop(_permit);
    vfs_dup_request(pid, file, oldfd, newfd).await
}

pub(crate) async fn sys_fstat(pid: u64, fd: usize, dst: u64) -> SyscallResult {
    let Some(file) = with_host(|_, procs| {
        let idx = find_proc(procs, pid)?;
        fd_file(&procs[idx], fd)
    }) else {
        return SyscallResult::err(EBADF);
    };
    vfs_fstat_request(pid, file, dst).await
}

pub(crate) async fn sys_chdir(pid: u64, path_ptr: u64) -> SyscallResult {
    let prepared = with_host(|alloc, procs| {
        let Some(idx) = find_proc(procs, pid) else {
            return Err(SyscallResult::err(ESRCH));
        };
        let child = &mut procs[idx];
        let mut path = [0u8; 128];
        let Some(len) = crate::child::copy_cstr_from_child(alloc, child, path_ptr, &mut path)
        else {
            return Err(SyscallResult::err(EFAULT));
        };
        let mut resolved = [0u8; MAX_PATH_BYTES];
        let Some(path_len) = resolve_child_path(child, &path[..len], &mut resolved) else {
            return Err(SyscallResult::err(ENAMETOOLONG));
        };
        let mut mrs = [0u64; 64];
        mrs[0] = IpcProtocol::HostToVfs.raw();
        mrs[1] = LINUX_ABI_VERSION;
        mrs[2] = child.cwd_inode as u64;
        mrs[3] = path_len as u64;
        pack_path_words(&resolved[..path_len], &mut mrs, 4);
        Ok((resolved, path_len, mrs, 4 + path_len.div_ceil(8)))
    });
    let (resolved, path_len, mrs, mr_len) = match prepared {
        Ok(values) => values,
        Err(err) => return err,
    };
    vfs_chdir_request(pid, resolved, path_len, VfsOp::Chdir.raw(), &mrs[..mr_len]).await
}

pub(crate) fn sys_getcwd(pid: u64, buf: u64, size: u64) -> SyscallResult {
    with_host(|alloc, procs| {
        let Some(idx) = find_proc(procs, pid) else {
            return SyscallResult::err(ESRCH);
        };
        let child = &procs[idx];
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
    })
}

pub(crate) async fn sys_pipe2(pid: u64, fds_ptr: u64, _flags: u32) -> SyscallResult {
    let Some((read_fd, write_fd)) = with_host(|_, procs| {
        let idx = find_proc(procs, pid)?;
        find_two_free_fds(&procs[idx])
    }) else {
        return SyscallResult::err(EMFILE);
    };
    vfs_pipe_request(pid, read_fd, write_fd, fds_ptr).await
}

pub(crate) async fn sys_unlinkat(pid: u64, dirfd: i32, path_ptr: u64, flags: u32) -> SyscallResult {
    let prepared = with_host(|alloc, procs| {
        let Some(idx) = find_proc(procs, pid) else {
            return Err(SyscallResult::err(ESRCH));
        };
        let child = &procs[idx];
        let mut path = [0u8; 128];
        let Some(len) = crate::child::copy_cstr_from_child(alloc, child, path_ptr, &mut path)
        else {
            return Err(SyscallResult::err(EFAULT));
        };
        if dirfd != AT_FDCWD
            && path.first() != Some(&b'/')
            && fd_file(child, dirfd as usize).is_none()
        {
            return Err(SyscallResult::err(EBADF));
        }
        let mut resolved = [0u8; MAX_PATH_BYTES];
        let Some(path_len) = resolve_child_path(child, &path[..len], &mut resolved) else {
            return Err(SyscallResult::err(ENAMETOOLONG));
        };
        let mut mrs = [0u64; 64];
        mrs[0] = IpcProtocol::HostToVfs.raw();
        mrs[1] = LINUX_ABI_VERSION;
        mrs[2] = path_len as u64;
        mrs[3] = flags as u64;
        pack_path_words(&resolved[..path_len], &mut mrs, 4);
        Ok((mrs, 4 + path_len.div_ceil(8)))
    });
    let (mrs, mr_len) = match prepared {
        Ok(values) => values,
        Err(err) => return err,
    };
    vfs_status_request(VfsOp::Unlink.raw(), &mrs[..mr_len]).await
}

pub(crate) async fn sys_mkdirat(pid: u64, dirfd: i32, path_ptr: u64, _mode: u32) -> SyscallResult {
    let prepared = with_host(|alloc, procs| {
        let Some(idx) = find_proc(procs, pid) else {
            return Err(SyscallResult::err(ESRCH));
        };
        let child = &procs[idx];
        let mut path = [0u8; 128];
        let Some(len) = crate::child::copy_cstr_from_child(alloc, child, path_ptr, &mut path)
        else {
            return Err(SyscallResult::err(EFAULT));
        };
        if dirfd != AT_FDCWD
            && path.first() != Some(&b'/')
            && fd_file(child, dirfd as usize).is_none()
        {
            return Err(SyscallResult::err(EBADF));
        }
        let mut resolved = [0u8; MAX_PATH_BYTES];
        let Some(path_len) = resolve_child_path(child, &path[..len], &mut resolved) else {
            return Err(SyscallResult::err(ENAMETOOLONG));
        };
        let mut mrs = [0u64; 64];
        mrs[0] = IpcProtocol::HostToVfs.raw();
        mrs[1] = LINUX_ABI_VERSION;
        mrs[2] = path_len as u64;
        pack_path_words(&resolved[..path_len], &mut mrs, 3);
        Ok((mrs, 3 + path_len.div_ceil(8)))
    });
    let (mrs, mr_len) = match prepared {
        Ok(values) => values,
        Err(err) => return err,
    };
    vfs_status_request(VfsOp::Mkdir.raw(), &mrs[..mr_len]).await
}

pub(crate) async fn sys_lseek(pid: u64, fd: usize, offset: i64, whence: u64) -> SyscallResult {
    let Some(file) = with_host(|_, procs| {
        let idx = find_proc(procs, pid)?;
        fd_file(&procs[idx], fd)
    }) else {
        return SyscallResult::err(EBADF);
    };
    vfs_lseek_request(file, offset, whence).await
}
