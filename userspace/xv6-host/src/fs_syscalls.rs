use crate::allocator::Allocator;
use crate::child::copy_cstr_from_child;
use crate::consts::*;
use crate::types::{SyscallResult, TaskStruct};
use crate::vfs::{
    fd_file, final_component_is_dot_or_dotdot, find_free_fd, find_two_free_fds, pack_path_words,
    resolve_child_path, start_vfs_chdir_request, start_vfs_close_request, start_vfs_dup_request,
    start_vfs_fstat_request, start_vfs_open_request, start_vfs_pipe_request,
    start_vfs_status_request,
};

pub(crate) fn sys_open(
    alloc: &mut Allocator,
    child: &mut TaskStruct,
    path_ptr: u64,
    flags: u32,
    syscall_mrs: &[u64; 64],
) -> SyscallResult {
    let mut path = [0u8; 128];
    let Some(len) = copy_cstr_from_child(alloc, child, path_ptr, &mut path) else {
        return SyscallResult::Reply(-1);
    };
    let mut resolved = [0u8; MAX_PATH_BYTES];
    let Some(path_len) = resolve_child_path(child, &path[..len], &mut resolved) else {
        return SyscallResult::Reply(-1);
    };
    let Some(fd) = find_free_fd(child) else {
        return SyscallResult::Reply(-1);
    };
    let mut mrs = [0u64; 64];
    mrs[0] = Xv6Protocol::HostToVfs.raw();
    mrs[1] = XV6_ABI_VERSION;
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
        return SyscallResult::Reply(-1);
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
        return SyscallResult::Reply(-1);
    };
    let Some(new_fd) = find_free_fd(child) else {
        return SyscallResult::Reply(-1);
    };
    start_vfs_dup_request(alloc, child, syscall_mrs, file, fd, new_fd)
}

pub(crate) fn sys_fstat(
    alloc: &mut Allocator,
    child: &mut TaskStruct,
    fd: usize,
    dst: u64,
    syscall_mrs: &[u64; 64],
) -> SyscallResult {
    let Some(file) = fd_file(child, fd) else {
        return SyscallResult::Reply(-1);
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
        return SyscallResult::Reply(-1);
    };
    let mut resolved = [0u8; MAX_PATH_BYTES];
    let Some(path_len) = resolve_child_path(child, &path[..len], &mut resolved) else {
        return SyscallResult::Reply(-1);
    };
    let mut mrs = [0u64; 64];
    mrs[0] = Xv6Protocol::HostToVfs.raw();
    mrs[1] = XV6_ABI_VERSION;
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

pub(crate) fn sys_pipe(
    alloc: &mut Allocator,
    child: &mut TaskStruct,
    fds_ptr: u64,
    syscall_mrs: &[u64; 64],
) -> SyscallResult {
    let Some((read_fd, write_fd)) = find_two_free_fds(child) else {
        return SyscallResult::Reply(-1);
    };
    start_vfs_pipe_request(alloc, child, syscall_mrs, read_fd, write_fd, fds_ptr)
}

pub(crate) fn sys_mknod(
    alloc: &mut Allocator,
    child: &mut TaskStruct,
    path_ptr: u64,
    major: u16,
    minor: u16,
    syscall_mrs: &[u64; 64],
) -> SyscallResult {
    let mut path = [0u8; 128];
    let Some(len) = copy_cstr_from_child(alloc, child, path_ptr, &mut path) else {
        return SyscallResult::Reply(-1);
    };
    let mut resolved = [0u8; MAX_PATH_BYTES];
    let Some(path_len) = resolve_child_path(child, &path[..len], &mut resolved) else {
        return SyscallResult::Reply(-1);
    };
    let mut mrs = [0u64; 64];
    mrs[0] = Xv6Protocol::HostToVfs.raw();
    mrs[1] = XV6_ABI_VERSION;
    mrs[2] = major as u64;
    mrs[3] = minor as u64;
    mrs[4] = path_len as u64;
    pack_path_words(&resolved[..path_len], &mut mrs, 5);
    start_vfs_status_request(
        alloc,
        child,
        syscall_mrs,
        VfsOp::Mknod.raw(),
        &mrs[..5 + path_len.div_ceil(8)],
    )
}

pub(crate) fn sys_unlink(
    alloc: &mut Allocator,
    child: &mut TaskStruct,
    path_ptr: u64,
    syscall_mrs: &[u64; 64],
) -> SyscallResult {
    let mut path = [0u8; 128];
    let Some(len) = copy_cstr_from_child(alloc, child, path_ptr, &mut path) else {
        return SyscallResult::Reply(-1);
    };
    if final_component_is_dot_or_dotdot(&path[..len]) {
        return SyscallResult::Reply(-1);
    }
    let mut resolved = [0u8; MAX_PATH_BYTES];
    let Some(path_len) = resolve_child_path(child, &path[..len], &mut resolved) else {
        return SyscallResult::Reply(-1);
    };
    let mut mrs = [0u64; 64];
    mrs[0] = Xv6Protocol::HostToVfs.raw();
    mrs[1] = XV6_ABI_VERSION;
    mrs[2] = path_len as u64;
    pack_path_words(&resolved[..path_len], &mut mrs, 3);
    start_vfs_status_request(
        alloc,
        child,
        syscall_mrs,
        VfsOp::Unlink.raw(),
        &mrs[..3 + path_len.div_ceil(8)],
    )
}

pub(crate) fn sys_link(
    alloc: &mut Allocator,
    child: &mut TaskStruct,
    old_ptr: u64,
    new_ptr: u64,
    syscall_mrs: &[u64; 64],
) -> SyscallResult {
    let mut old_path = [0u8; 128];
    let mut new_path = [0u8; 128];
    let Some(old_len) = copy_cstr_from_child(alloc, child, old_ptr, &mut old_path) else {
        return SyscallResult::Reply(-1);
    };
    let Some(new_len) = copy_cstr_from_child(alloc, child, new_ptr, &mut new_path) else {
        return SyscallResult::Reply(-1);
    };
    if old_len == 0 || new_len == 0 {
        return SyscallResult::Reply(-1);
    }
    let mut old_resolved = [0u8; MAX_PATH_BYTES];
    let mut new_resolved = [0u8; MAX_PATH_BYTES];
    let Some(old_path_len) = resolve_child_path(child, &old_path[..old_len], &mut old_resolved)
    else {
        return SyscallResult::Reply(-1);
    };
    let Some(new_path_len) = resolve_child_path(child, &new_path[..new_len], &mut new_resolved)
    else {
        return SyscallResult::Reply(-1);
    };
    let old_words = old_path_len.div_ceil(8);
    let new_words = new_path_len.div_ceil(8);
    let mut mrs = [0u64; 64];
    if 4 + old_words + new_words > mrs.len() {
        return SyscallResult::Reply(-1);
    }
    mrs[0] = Xv6Protocol::HostToVfs.raw();
    mrs[1] = XV6_ABI_VERSION;
    mrs[2] = old_path_len as u64;
    mrs[3] = new_path_len as u64;
    pack_path_words(&old_resolved[..old_path_len], &mut mrs, 4);
    pack_path_words(&new_resolved[..new_path_len], &mut mrs, 4 + old_words);
    start_vfs_status_request(
        alloc,
        child,
        syscall_mrs,
        VfsOp::Link.raw(),
        &mrs[..4 + old_words + new_words],
    )
}

pub(crate) fn sys_mkdir(
    alloc: &mut Allocator,
    child: &mut TaskStruct,
    path_ptr: u64,
    syscall_mrs: &[u64; 64],
) -> SyscallResult {
    let mut path = [0u8; 128];
    let Some(len) = copy_cstr_from_child(alloc, child, path_ptr, &mut path) else {
        return SyscallResult::Reply(-1);
    };
    let mut resolved = [0u8; MAX_PATH_BYTES];
    let Some(path_len) = resolve_child_path(child, &path[..len], &mut resolved) else {
        return SyscallResult::Reply(-1);
    };
    let mut mrs = [0u64; 64];
    mrs[0] = Xv6Protocol::HostToVfs.raw();
    mrs[1] = XV6_ABI_VERSION;
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
