use core::cmp::min;
use core::future::Future;

use linux_abi::{
    CONSOLE_INO, EBADF, EINVAL, ENOSYS, FileKind, IpcStatus, MAX_IO_BYTES, ROOT_INO, VfsOp,
    open_readable, open_writable, pack_stat_kind_nlink,
};
use sel4_user::{IpcMessage, info, msg_label, warn};

use crate::console::{init_console, read_console, write_console};
use crate::cpio;
use crate::ipc::{
    err, err_code, host_request, ok, path_mrs_valid, send_host_async_reply, valid_host,
    with_shared_buffer, with_shared_buffer_mut,
};
use crate::pipe::{handle_pipe, read_pipe, write_pipe};
use crate::ramfs;
use crate::state::{
    FILE_CONSOLE, FILE_PIPE_READ, FILE_PIPE_WRITE, FILE_RAM_DIR, FILE_RAM_FILE, ReleaseResult,
    acquire_file_io, add_file_offset, alloc_file, detach_file, file_snapshot, release_file,
    release_file_io, reset_all, retain_file, set_file_offset, valid_file,
};

pub(crate) enum RequestResult {
    Reply([u64; 4]),
    Deferred,
}

pub(crate) async fn handle_request(msg: &IpcMessage) -> RequestResult {
    let raw_op = msg_label(msg.info);
    match VfsOp::from_raw(raw_op) {
        Some(VfsOp::Init) => RequestResult::Reply(handle_init(msg)),
        Some(VfsOp::ProcInit) => RequestResult::Reply(handle_proc_init(msg)),
        Some(VfsOp::ProcFork) => RequestResult::Reply(handle_proc_fork(msg)),
        Some(VfsOp::ProcExit) => RequestResult::Reply(handle_proc_exit(msg)),
        Some(VfsOp::Open) => handle_host_request(*msg, handle_open_async).await,
        Some(VfsOp::Close) => handle_host_request(*msg, handle_close_async).await,
        Some(VfsOp::Dup) => handle_host_request(*msg, handle_dup_async).await,
        Some(VfsOp::Read) => handle_host_request(*msg, handle_read_async).await,
        Some(VfsOp::Write) => handle_host_request(*msg, handle_write_async).await,
        Some(VfsOp::Fstat) => handle_host_request(*msg, handle_fstat_async).await,
        Some(VfsOp::Chdir) => handle_host_request(*msg, handle_chdir_async).await,
        Some(VfsOp::Pipe) => handle_host_request(*msg, handle_pipe_async).await,
        Some(VfsOp::Unlink) => handle_host_request(*msg, handle_unlink_async).await,
        Some(VfsOp::Mkdir) => handle_host_request(*msg, handle_mkdir_async).await,
        Some(VfsOp::ExecOpen) => handle_host_request(*msg, handle_exec_open_async).await,
        Some(VfsOp::ExecRead) => handle_host_request(*msg, handle_exec_read_async).await,
        Some(VfsOp::ExecClose) => handle_host_request(*msg, handle_exec_close_async).await,
        Some(VfsOp::Getcwd) => RequestResult::Reply(ok()),
        Some(VfsOp::Lseek) => handle_host_request(*msg, handle_lseek_async).await,
        None => {
            warn!("vfs-server: unsupported op={}", raw_op);
            RequestResult::Reply(err_code(ENOSYS))
        }
    }
}

async fn handle_host_request<F, Fut>(msg: IpcMessage, handler: F) -> RequestResult
where
    F: FnOnce(IpcMessage) -> Fut,
    Fut: Future<Output = [u64; 4]>,
{
    let Some(request) = host_request(&msg) else {
        return RequestResult::Reply(err());
    };
    let reply = handler(msg).await;
    if request.async_request {
        send_host_async_reply(request.request_id, reply);
        RequestResult::Deferred
    } else {
        RequestResult::Reply(reply)
    }
}

fn handle_init(msg: &IpcMessage) -> [u64; 4] {
    if !valid_host(msg) {
        return err();
    }
    reset_all();
    ramfs::reset();
    if !ramfs::init_root() {
        return err();
    }
    if !cpio::unpack(crate::rootfs_bytes()) {
        warn!("vfs-server: rootfs unpack failed");
        return err();
    }
    if ramfs::ensure_dir(b"/tmp").is_err() || ramfs::install_console().is_err() {
        return err();
    }
    if !init_console() {
        return err();
    }
    info!("vfs-server: ramfs init complete");
    ok()
}

fn handle_proc_init(msg: &IpcMessage) -> [u64; 4] {
    if !valid_host(msg) {
        return err();
    }
    if !ramfs::retain(ROOT_INO) {
        return err();
    }
    let Some(stdin_file) = alloc_file(FILE_CONSOLE, CONSOLE_INO, 0, true, true) else {
        let _ = ramfs::release(ROOT_INO);
        return err();
    };
    let Some(stdout_file) = alloc_file(FILE_CONSOLE, CONSOLE_INO, 0, true, true) else {
        release_file(stdin_file);
        let _ = ramfs::release(ROOT_INO);
        return err();
    };
    let Some(stderr_file) = alloc_file(FILE_CONSOLE, CONSOLE_INO, 0, true, true) else {
        release_file(stdin_file);
        release_file(stdout_file);
        let _ = ramfs::release(ROOT_INO);
        return err();
    };
    [
        IpcStatus::Ok.raw(),
        stdin_file as u64,
        stdout_file as u64,
        stderr_file as u64,
    ]
}

fn handle_proc_fork(msg: &IpcMessage) -> [u64; 4] {
    if !valid_host(msg) {
        return err();
    }
    let cwd_inum = msg.mrs[2] as u32;
    if cwd_inum == 0 || ramfs::retain(cwd_inum) {
        ok()
    } else {
        err()
    }
}

fn handle_proc_exit(msg: &IpcMessage) -> [u64; 4] {
    if !valid_host(msg) {
        return err();
    }
    let cwd_inum = msg.mrs[2] as u32;
    if cwd_inum == 0 || ramfs::release(cwd_inum) {
        ok()
    } else {
        err()
    }
}

async fn handle_open_async(msg: IpcMessage) -> [u64; 4] {
    if !valid_host(&msg) {
        return err();
    }
    let flags = msg.mrs[2] as u32;
    let path_len = msg.mrs[3] as usize;
    if !path_mrs_valid(&msg, 4, path_len) {
        return err();
    }
    let Some(path) = ramfs::path_from_words(&msg.mrs, 4, path_len) else {
        return err();
    };
    match ramfs::open_path(&path[..path_len], flags) {
        Ok((inum, kind, size)) => {
            let (file_kind, node) = match kind {
                FileKind::File => (FILE_RAM_FILE, inum),
                FileKind::Directory => (FILE_RAM_DIR, inum),
                FileKind::Device => {
                    let _ = ramfs::release(inum);
                    (FILE_CONSOLE, CONSOLE_INO)
                }
            };
            let Some(file) = alloc_file(
                file_kind,
                node,
                0,
                open_readable(flags),
                open_writable(flags),
            ) else {
                if file_kind != FILE_CONSOLE {
                    let _ = ramfs::release(inum);
                }
                return err();
            };
            [
                IpcStatus::Ok.raw(),
                file as u64,
                kind.raw() as u64,
                size as u64,
            ]
        }
        Err(e) => err_code(e),
    }
}

async fn handle_close_async(msg: IpcMessage) -> [u64; 4] {
    if !valid_host(&msg) {
        return err();
    }
    match detach_file(msg.mrs[2] as usize) {
        ReleaseResult::Invalid => err_code(EBADF),
        ReleaseResult::Done => ok(),
        ReleaseResult::Inode(inum) => {
            if ramfs::release(inum) {
                ok()
            } else {
                err()
            }
        }
    }
}

async fn handle_dup_async(msg: IpcMessage) -> [u64; 4] {
    if !valid_host(&msg) || !retain_file(msg.mrs[2] as usize) {
        return err_code(EBADF);
    }
    [IpcStatus::Ok.raw(), msg.mrs[2], 0, 0]
}

async fn handle_pipe_async(msg: IpcMessage) -> [u64; 4] {
    handle_pipe(&msg)
}

async fn handle_read_async(msg: IpcMessage) -> [u64; 4] {
    if !valid_host(&msg) {
        return err();
    }
    let Some(file_idx) = valid_file(msg.mrs[2] as usize) else {
        return err_code(EBADF);
    };
    let max_len = min(msg.mrs[3] as usize, MAX_IO_BYTES);
    let Some(file) = file_snapshot(file_idx) else {
        return err_code(EBADF);
    };
    if !file.readable {
        return err_code(EBADF);
    }
    match file.kind {
        FILE_RAM_FILE | FILE_RAM_DIR => read_ram_file(file_idx, max_len),
        FILE_PIPE_READ => read_pipe(file.aux, max_len),
        FILE_CONSOLE => read_console(max_len).await,
        _ => err(),
    }
}

async fn handle_write_async(msg: IpcMessage) -> [u64; 4] {
    if !valid_host(&msg) {
        return err();
    }
    let Some(file_idx) = valid_file(msg.mrs[2] as usize) else {
        return err_code(EBADF);
    };
    let max_len = min(msg.mrs[3] as usize, MAX_IO_BYTES);
    let Some(file) = file_snapshot(file_idx) else {
        return err_code(EBADF);
    };
    if !file.writable {
        return err_code(EBADF);
    }
    match file.kind {
        FILE_RAM_FILE => write_ram_file(file_idx, max_len),
        FILE_PIPE_WRITE => write_pipe(file.aux, max_len),
        FILE_CONSOLE => write_console(max_len).await,
        _ => err(),
    }
}

async fn handle_fstat_async(msg: IpcMessage) -> [u64; 4] {
    if !valid_host(&msg) {
        return err();
    }
    let Some(file_idx) = valid_file(msg.mrs[2] as usize) else {
        return err_code(EBADF);
    };
    let Some(file) = file_snapshot(file_idx) else {
        return err_code(EBADF);
    };
    match file.kind {
        FILE_RAM_FILE | FILE_RAM_DIR => {
            let Some(node) = ramfs::inode(file.node) else {
                return err();
            };
            [
                IpcStatus::Ok.raw(),
                pack_stat_kind_nlink(node.kind.raw(), node.nlink),
                file.node as u64,
                node.size as u64,
            ]
        }
        FILE_PIPE_READ | FILE_PIPE_WRITE => [
            IpcStatus::Ok.raw(),
            pack_stat_kind_nlink(FileKind::File.raw(), 1),
            4 + file.aux as u64,
            1,
        ],
        FILE_CONSOLE => [
            IpcStatus::Ok.raw(),
            pack_stat_kind_nlink(FileKind::Device.raw(), 1),
            CONSOLE_INO as u64,
            1,
        ],
        _ => err(),
    }
}

async fn handle_chdir_async(msg: IpcMessage) -> [u64; 4] {
    if !valid_host(&msg) {
        return err();
    }
    let old_cwd = msg.mrs[2] as u32;
    let path_len = msg.mrs[3] as usize;
    if !path_mrs_valid(&msg, 4, path_len) {
        return err();
    }
    let Some(path) = ramfs::path_from_words(&msg.mrs, 4, path_len) else {
        return err();
    };
    match ramfs::walk(&path[..path_len]) {
        Ok(inum) => {
            let Some(node) = ramfs::inode(inum) else {
                return err();
            };
            if node.kind != FileKind::Directory {
                return err_code(linux_abi::ENOTDIR);
            }
            if !ramfs::retain(inum) {
                return err();
            }
            if old_cwd != 0 {
                let _ = ramfs::release(old_cwd);
            }
            [IpcStatus::Ok.raw(), inum as u64, 0, 0]
        }
        Err(e) => err_code(e),
    }
}

async fn handle_unlink_async(msg: IpcMessage) -> [u64; 4] {
    if !valid_host(&msg) {
        return err();
    }
    let path_len = msg.mrs[2] as usize;
    let flags = msg.mrs[3] as u32;
    if !path_mrs_valid(&msg, 4, path_len) {
        return err();
    }
    let Some(path) = ramfs::path_from_words(&msg.mrs, 4, path_len) else {
        return err();
    };
    match ramfs::unlink(&path[..path_len], flags) {
        Ok(()) => ok(),
        Err(e) => err_code(e),
    }
}

async fn handle_mkdir_async(msg: IpcMessage) -> [u64; 4] {
    if !valid_host(&msg) {
        return err();
    }
    let path_len = msg.mrs[2] as usize;
    if !path_mrs_valid(&msg, 3, path_len) {
        return err();
    }
    let Some(path) = ramfs::path_from_words(&msg.mrs, 3, path_len) else {
        return err();
    };
    match ramfs::mkdir(&path[..path_len]) {
        Ok(_) => ok(),
        Err(e) => err_code(e),
    }
}

async fn handle_exec_open_async(msg: IpcMessage) -> [u64; 4] {
    if !valid_host(&msg) {
        return err();
    }
    let path_len = msg.mrs[2] as usize;
    if !path_mrs_valid(&msg, 3, path_len) {
        return err();
    }
    let Some(path) = ramfs::path_from_words(&msg.mrs, 3, path_len) else {
        return err();
    };
    match ramfs::open_path(&path[..path_len], 0) {
        Ok((inum, FileKind::File, size)) => [IpcStatus::Ok.raw(), inum as u64, size as u64, 0],
        Ok((inum, _, _)) => {
            let _ = ramfs::release(inum);
            err()
        }
        Err(e) => err_code(e),
    }
}

async fn handle_exec_read_async(msg: IpcMessage) -> [u64; 4] {
    if !valid_host(&msg) {
        return err();
    }
    let inum = msg.mrs[2] as u32;
    let offset = msg.mrs[3] as usize;
    let request = min(msg.mrs[5] as usize, MAX_IO_BYTES);
    with_shared_buffer_mut(
        |dst| match ramfs::read_inode(inum, offset, &mut dst[..request]) {
            Ok(n) => [IpcStatus::Ok.raw(), n as u64, 0, 0],
            Err(e) => err_code(e),
        },
    )
}

async fn handle_exec_close_async(msg: IpcMessage) -> [u64; 4] {
    if !valid_host(&msg) {
        return err();
    }
    if ramfs::release(msg.mrs[2] as u32) {
        ok()
    } else {
        err()
    }
}

async fn handle_lseek_async(msg: IpcMessage) -> [u64; 4] {
    if !valid_host(&msg) {
        return err();
    }
    let Some(file_idx) = valid_file(msg.mrs[2] as usize) else {
        return err_code(EBADF);
    };
    let Some(file) = file_snapshot(file_idx) else {
        return err_code(EBADF);
    };
    if file.kind != FILE_RAM_FILE && file.kind != FILE_RAM_DIR {
        return err_code(EINVAL);
    }
    match ramfs::seek(file.node, file.offset, msg.mrs[3] as i64, msg.mrs[4]) {
        Ok(next) => {
            set_file_offset(file_idx, next);
            [IpcStatus::Ok.raw(), next as u64, 0, 0]
        }
        Err(e) => err_code(e),
    }
}

fn read_ram_file(file_idx: usize, max_len: usize) -> [u64; 4] {
    if !acquire_file_io(file_idx) {
        return err();
    }
    let Some(file) = file_snapshot(file_idx) else {
        release_file_io(file_idx);
        return err();
    };
    let reply = with_shared_buffer_mut(|dst| {
        match ramfs::read_inode(file.node, file.offset, &mut dst[..max_len]) {
            Ok(n) => {
                add_file_offset(file_idx, n);
                [IpcStatus::Ok.raw(), n as u64, file.kind as u64, 0]
            }
            Err(e) => err_code(e),
        }
    });
    release_file_io(file_idx);
    reply
}

fn write_ram_file(file_idx: usize, max_len: usize) -> [u64; 4] {
    if !acquire_file_io(file_idx) {
        return err();
    }
    let Some(file) = file_snapshot(file_idx) else {
        release_file_io(file_idx);
        return err();
    };
    let reply = with_shared_buffer(|src| {
        match ramfs::write_inode(file.node, file.offset, &src[..max_len]) {
            Ok(n) => {
                add_file_offset(file_idx, n);
                [
                    IpcStatus::Ok.raw(),
                    n as u64,
                    FileKind::File.raw() as u64,
                    0,
                ]
            }
            Err(e) => err_code(e),
        }
    });
    release_file_io(file_idx);
    reply
}
