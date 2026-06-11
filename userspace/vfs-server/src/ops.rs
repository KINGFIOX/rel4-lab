use core::cmp::min;
use core::future::Future;

use sel4_user::{IpcMessage, info, msg_label, warn};
use xv6_abi::{
    CONSOLE_INO, FS_BLOCK_SIZE, ROOT_INO, VfsOp, XV6_ABI_VERSION, XV6_MAX_FILE_WRITE, Xv6FileType,
    Xv6FsOp, Xv6OpenFlag, Xv6Protocol, Xv6Status, pack_stat_type_nlink,
};

use crate::console::{init_console, read_console, write_console};
use crate::ipc::{
    copy_path_words, err, host_request, ok, path_mrs_valid, reply4, send_host_async_reply,
    valid_host, xv6fs_call, xv6fs_call_async, xv6fs_release, xv6fs_release_async, xv6fs_retain,
    xv6fs_retain_async,
};
use crate::pipe::{handle_pipe, read_pipe, write_pipe};
use crate::state::{
    FILE_CONSOLE, FILE_PIPE_READ, FILE_PIPE_WRITE, FILE_XV6_DIR, FILE_XV6_FILE, ReleaseResult,
    acquire_file_io, add_file_offset, alloc_file, detach_file, file_snapshot, release_file,
    release_file_io, reset_all, retain_file, valid_file,
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
        Some(VfsOp::Mknod) => handle_host_request(*msg, handle_mknod_async).await,
        Some(VfsOp::Unlink) => handle_host_request(*msg, handle_unlink_async).await,
        Some(VfsOp::Link) => handle_host_request(*msg, handle_link_async).await,
        Some(VfsOp::Mkdir) => handle_host_request(*msg, handle_mkdir_async).await,
        Some(VfsOp::ExecOpen) => handle_host_request(*msg, handle_exec_open_async).await,
        Some(VfsOp::ExecRead) => handle_host_request(*msg, handle_exec_read_async).await,
        Some(VfsOp::ExecClose) => handle_host_request(*msg, handle_exec_close_async).await,
        None => {
            warn!("vfs-server: unsupported op={}", raw_op);
            RequestResult::Reply([Xv6Status::NoSyscall.raw(), 0, 0, 0])
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
    if !init_console() {
        return err();
    }
    let Some(reply) = xv6fs_call(
        Xv6FsOp::Init.raw(),
        &[Xv6Protocol::VfsToXv6Fs.raw(), XV6_ABI_VERSION],
    ) else {
        return err();
    };
    if reply.mrs[0] == Xv6Status::Ok.raw() {
        info!("vfs-server: init complete");
    }
    reply4(&reply)
}

fn handle_proc_init(msg: &IpcMessage) -> [u64; 4] {
    if !valid_host(msg) {
        return err();
    }
    if !xv6fs_retain(ROOT_INO) {
        return err();
    }
    let Some(stdin_file) = alloc_file(FILE_CONSOLE, CONSOLE_INO, 0, true, true) else {
        let _ = xv6fs_release(ROOT_INO);
        return err();
    };
    let Some(stdout_file) = alloc_file(FILE_CONSOLE, CONSOLE_INO, 0, true, true) else {
        release_file(stdin_file);
        let _ = xv6fs_release(ROOT_INO);
        return err();
    };
    let Some(stderr_file) = alloc_file(FILE_CONSOLE, CONSOLE_INO, 0, true, true) else {
        release_file(stdin_file);
        release_file(stdout_file);
        let _ = xv6fs_release(ROOT_INO);
        return err();
    };
    [
        Xv6Status::Ok.raw(),
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
    if cwd_inum == 0 || xv6fs_retain(cwd_inum) {
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
    if cwd_inum == 0 || xv6fs_release(cwd_inum) {
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
    let mut mrs = [0u64; 64];
    mrs[0] = Xv6Protocol::VfsToXv6Fs.raw();
    mrs[1] = XV6_ABI_VERSION;
    mrs[2] = ROOT_INO as u64;
    mrs[3] = flags as u64;
    mrs[4] = path_len as u64;
    copy_path_words(&msg, 4, path_len, &mut mrs, 5);
    let Some(reply) =
        xv6fs_call_async(Xv6FsOp::OpenAt.raw(), &mrs[..5 + path_len.div_ceil(8)]).await
    else {
        return err();
    };
    if reply[0] != Xv6Status::Ok.raw() {
        return reply;
    }
    let inum = reply[1] as u32;
    let typ = reply[2] as u16;
    let wants_write = flags
        & (Xv6OpenFlag::WriteOnly.raw()
            | Xv6OpenFlag::ReadWrite.raw()
            | Xv6OpenFlag::Create.raw()
            | Xv6OpenFlag::Truncate.raw())
        != 0;
    let readable =
        flags & Xv6OpenFlag::WriteOnly.raw() == 0 || flags & Xv6OpenFlag::ReadWrite.raw() != 0;
    let writable = flags & (Xv6OpenFlag::WriteOnly.raw() | Xv6OpenFlag::ReadWrite.raw()) != 0;
    let (kind, node) = match Xv6FileType::from_raw(typ) {
        Some(Xv6FileType::File) => (FILE_XV6_FILE, inum),
        Some(Xv6FileType::Directory) if !wants_write => (FILE_XV6_DIR, inum),
        Some(Xv6FileType::Device) => {
            let _ = xv6fs_release_async(inum).await;
            (FILE_CONSOLE, CONSOLE_INO)
        }
        _ => {
            let _ = xv6fs_release_async(inum).await;
            return err();
        }
    };
    let Some(file) = alloc_file(kind, node, 0, readable, writable) else {
        if kind != FILE_CONSOLE {
            let _ = xv6fs_release_async(inum).await;
        }
        return err();
    };
    [Xv6Status::Ok.raw(), file as u64, typ as u64, reply[3]]
}

async fn handle_close_async(msg: IpcMessage) -> [u64; 4] {
    if !valid_host(&msg) {
        return err();
    }
    match detach_file(msg.mrs[2] as usize) {
        ReleaseResult::Invalid => err(),
        ReleaseResult::Done => ok(),
        ReleaseResult::Xv6(inum) => {
            if xv6fs_release_async(inum).await {
                ok()
            } else {
                err()
            }
        }
    }
}

async fn handle_dup_async(msg: IpcMessage) -> [u64; 4] {
    if !valid_host(&msg) || !retain_file(msg.mrs[2] as usize) {
        return err();
    }
    [Xv6Status::Ok.raw(), msg.mrs[2], 0, 0]
}

async fn handle_pipe_async(msg: IpcMessage) -> [u64; 4] {
    handle_pipe(&msg)
}

async fn handle_read_async(msg: IpcMessage) -> [u64; 4] {
    if !valid_host(&msg) {
        return err();
    }
    let Some(file_idx) = valid_file(msg.mrs[2] as usize) else {
        return err();
    };
    let max_len = min(msg.mrs[3] as usize, FS_BLOCK_SIZE);
    let Some(file) = file_snapshot(file_idx) else {
        return err();
    };
    if !file.readable {
        return err();
    }
    match file.kind {
        FILE_XV6_FILE | FILE_XV6_DIR => read_xv6_file_async(file_idx, max_len).await,
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
        return err();
    };
    let max_len = min(msg.mrs[3] as usize, XV6_MAX_FILE_WRITE);
    let Some(file) = file_snapshot(file_idx) else {
        return err();
    };
    if !file.writable {
        return err();
    }
    match file.kind {
        FILE_XV6_FILE => write_xv6_file_async(file_idx, max_len).await,
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
        return err();
    };
    let Some(file) = file_snapshot(file_idx) else {
        return err();
    };
    match file.kind {
        FILE_XV6_FILE | FILE_XV6_DIR => {
            let Some(reply) = xv6fs_call_async(
                Xv6FsOp::Fstat.raw(),
                &[
                    Xv6Protocol::VfsToXv6Fs.raw(),
                    XV6_ABI_VERSION,
                    file.node as u64,
                ],
            )
            .await
            else {
                return err();
            };
            if reply[0] != Xv6Status::Ok.raw() {
                return reply;
            }
            [
                Xv6Status::Ok.raw(),
                pack_stat_type_nlink(reply[1] as u16, reply[2] as u16),
                file.node as u64,
                reply[3],
            ]
        }
        FILE_PIPE_READ | FILE_PIPE_WRITE => [
            Xv6Status::Ok.raw(),
            pack_stat_type_nlink(Xv6FileType::File.raw(), 1),
            4 + file.aux as u64,
            1,
        ],
        FILE_CONSOLE => [
            Xv6Status::Ok.raw(),
            pack_stat_type_nlink(Xv6FileType::Device.raw(), 1),
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
    let old_cwd_inum = msg.mrs[2] as u32;
    let path_len = msg.mrs[3] as usize;
    if !path_mrs_valid(&msg, 4, path_len) {
        return err();
    }
    let mut mrs = [0u64; 64];
    mrs[0] = Xv6Protocol::VfsToXv6Fs.raw();
    mrs[1] = XV6_ABI_VERSION;
    mrs[2] = ROOT_INO as u64;
    mrs[4] = path_len as u64;
    copy_path_words(&msg, 4, path_len, &mut mrs, 5);
    let Some(reply) = xv6fs_call_async(
        Xv6FsOp::LookupDirectory.raw(),
        &mrs[..5 + path_len.div_ceil(8)],
    )
    .await
    else {
        return err();
    };
    if reply[0] != Xv6Status::Ok.raw() {
        reply
    } else {
        let new_cwd_inum = reply[1] as u32;
        if !xv6fs_retain_async(new_cwd_inum).await {
            return err();
        }
        if old_cwd_inum != 0 && !xv6fs_release_async(old_cwd_inum).await {
            let _ = xv6fs_release_async(new_cwd_inum).await;
            return err();
        }
        [Xv6Status::Ok.raw(), new_cwd_inum as u64, 0, 0]
    }
}

async fn handle_mknod_async(msg: IpcMessage) -> [u64; 4] {
    if !valid_host(&msg) {
        return err();
    }
    let path_len = msg.mrs[4] as usize;
    if !path_mrs_valid(&msg, 5, path_len) {
        return err();
    }
    let mut mrs = [0u64; 64];
    mrs[0] = Xv6Protocol::VfsToXv6Fs.raw();
    mrs[1] = XV6_ABI_VERSION;
    mrs[2] = ROOT_INO as u64;
    mrs[3] = msg.mrs[2];
    mrs[4] = msg.mrs[3];
    mrs[5] = path_len as u64;
    copy_path_words(&msg, 5, path_len, &mut mrs, 6);
    xv6fs_call_async(Xv6FsOp::Mknod.raw(), &mrs[..6 + path_len.div_ceil(8)])
        .await
        .unwrap_or_else(err)
}

async fn handle_unlink_async(msg: IpcMessage) -> [u64; 4] {
    namespace_one_path_async(msg, Xv6FsOp::Unlink.raw()).await
}

async fn handle_mkdir_async(msg: IpcMessage) -> [u64; 4] {
    namespace_one_path_async(msg, Xv6FsOp::Mkdir.raw()).await
}

async fn namespace_one_path_async(msg: IpcMessage, op: u64) -> [u64; 4] {
    if !valid_host(&msg) {
        return err();
    }
    let path_len = msg.mrs[2] as usize;
    if !path_mrs_valid(&msg, 3, path_len) {
        return err();
    }
    let mut mrs = [0u64; 64];
    mrs[0] = Xv6Protocol::VfsToXv6Fs.raw();
    mrs[1] = XV6_ABI_VERSION;
    mrs[2] = ROOT_INO as u64;
    mrs[3] = path_len as u64;
    copy_path_words(&msg, 3, path_len, &mut mrs, 4);
    xv6fs_call_async(op, &mrs[..4 + path_len.div_ceil(8)])
        .await
        .unwrap_or_else(err)
}

async fn handle_link_async(msg: IpcMessage) -> [u64; 4] {
    if !valid_host(&msg) {
        return err();
    }
    let old_len = msg.mrs[2] as usize;
    let new_len = msg.mrs[3] as usize;
    let old_words = old_len.div_ceil(8);
    if !path_mrs_valid(&msg, 4, old_len) || !path_mrs_valid(&msg, 4 + old_words, new_len) {
        return err();
    }
    let mut mrs = [0u64; 64];
    if 5 + old_words + new_len.div_ceil(8) > mrs.len() {
        return err();
    }
    mrs[0] = Xv6Protocol::VfsToXv6Fs.raw();
    mrs[1] = XV6_ABI_VERSION;
    mrs[2] = ROOT_INO as u64;
    mrs[3] = old_len as u64;
    mrs[4] = new_len as u64;
    copy_path_words(&msg, 4, old_len, &mut mrs, 5);
    copy_path_words(&msg, 4 + old_words, new_len, &mut mrs, 5 + old_words);
    xv6fs_call_async(
        Xv6FsOp::Link.raw(),
        &mrs[..5 + old_words + new_len.div_ceil(8)],
    )
    .await
    .unwrap_or_else(err)
}

async fn handle_exec_open_async(msg: IpcMessage) -> [u64; 4] {
    if !valid_host(&msg) {
        return err();
    }
    let path_len = msg.mrs[2] as usize;
    if !path_mrs_valid(&msg, 3, path_len) {
        return err();
    }
    let mut mrs = [0u64; 64];
    mrs[0] = Xv6Protocol::VfsToXv6Fs.raw();
    mrs[1] = XV6_ABI_VERSION;
    mrs[2] = ROOT_INO as u64;
    mrs[3] = 0;
    mrs[4] = path_len as u64;
    copy_path_words(&msg, 3, path_len, &mut mrs, 5);
    let Some(reply) =
        xv6fs_call_async(Xv6FsOp::OpenAt.raw(), &mrs[..5 + path_len.div_ceil(8)]).await
    else {
        return err();
    };
    if reply[0] != Xv6Status::Ok.raw() || reply[2] != Xv6FileType::File.raw() as u64 {
        if reply[0] == Xv6Status::Ok.raw() {
            let _ = xv6fs_release_async(reply[1] as u32).await;
        }
        return err();
    }
    [Xv6Status::Ok.raw(), reply[1], reply[3], 0]
}

async fn handle_exec_read_async(msg: IpcMessage) -> [u64; 4] {
    if !valid_host(&msg) {
        return err();
    }
    let request = min(msg.mrs[5] as usize, FS_BLOCK_SIZE);
    xv6fs_call_async(
        Xv6FsOp::Read.raw(),
        &[
            Xv6Protocol::VfsToXv6Fs.raw(),
            XV6_ABI_VERSION,
            msg.mrs[2],
            msg.mrs[3],
            request as u64,
        ],
    )
    .await
    .unwrap_or_else(err)
}

async fn handle_exec_close_async(msg: IpcMessage) -> [u64; 4] {
    if !valid_host(&msg) {
        return err();
    }
    if xv6fs_release_async(msg.mrs[2] as u32).await {
        ok()
    } else {
        err()
    }
}

async fn read_xv6_file_async(file_idx: usize, max_len: usize) -> [u64; 4] {
    if !acquire_file_io(file_idx) {
        return err();
    }
    let Some(file) = file_snapshot(file_idx) else {
        release_file_io(file_idx);
        return err();
    };
    let op = if file.kind == FILE_XV6_DIR {
        Xv6FsOp::ReadDir.raw()
    } else {
        Xv6FsOp::Read.raw()
    };
    let Some(reply) = xv6fs_call_async(
        op,
        &[
            Xv6Protocol::VfsToXv6Fs.raw(),
            XV6_ABI_VERSION,
            file.node as u64,
            file.offset as u64,
            max_len as u64,
        ],
    )
    .await
    else {
        release_file_io(file_idx);
        return err();
    };
    if reply[0] == Xv6Status::Ok.raw() {
        add_file_offset(file_idx, reply[1] as usize);
    }
    release_file_io(file_idx);
    reply
}

async fn write_xv6_file_async(file_idx: usize, max_len: usize) -> [u64; 4] {
    if !acquire_file_io(file_idx) {
        return err();
    }
    let Some(file) = file_snapshot(file_idx) else {
        release_file_io(file_idx);
        return err();
    };
    let Some(reply) = xv6fs_call_async(
        Xv6FsOp::Write.raw(),
        &[
            Xv6Protocol::VfsToXv6Fs.raw(),
            XV6_ABI_VERSION,
            file.node as u64,
            file.offset as u64,
            max_len as u64,
        ],
    )
    .await
    else {
        release_file_io(file_idx);
        return err();
    };
    if reply[0] == Xv6Status::Ok.raw() {
        add_file_offset(file_idx, reply[1] as usize);
    }
    release_file_io(file_idx);
    reply
}
