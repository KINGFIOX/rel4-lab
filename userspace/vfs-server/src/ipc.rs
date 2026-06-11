use sel4_user::{IpcMessage, msg_info, msg_label, msg_len, sel4_call, sel4_send, sel4_yield, warn};
use xv6_abi::{
    MAX_PATH_BYTES, XV6_ABI_VERSION, XV6_HOST_REPLY_ENDPOINT_CPTR, XV6_MAX_FILE_WRITE,
    XV6_XV6FS_ENDPOINT_CPTR, Xv6FsOp, Xv6Protocol, Xv6Status,
};

const FS_BUSY_RETRY_LIMIT: usize = 4096;

#[derive(Copy, Clone)]
pub(crate) struct HostRequest {
    pub(crate) async_request: bool,
    pub(crate) request_id: u64,
}

impl HostRequest {
    const fn sync() -> Self {
        Self {
            async_request: false,
            request_id: 0,
        }
    }
}

pub(crate) fn valid_host(msg: &IpcMessage) -> bool {
    host_request(msg).is_some()
}

pub(crate) fn host_request(msg: &IpcMessage) -> Option<HostRequest> {
    if msg.mrs[0] == Xv6Protocol::HostToVfs.raw() && msg.mrs[1] == XV6_ABI_VERSION {
        Some(HostRequest::sync())
    } else if msg.mrs[0] == Xv6Protocol::HostToVfsAsync.raw() && msg.mrs[1] != 0 {
        Some(HostRequest {
            async_request: true,
            request_id: msg.mrs[1],
        })
    } else {
        None
    }
}

pub(crate) fn ok() -> [u64; 4] {
    [Xv6Status::Ok.raw(), 0, 0, 0]
}

pub(crate) fn err() -> [u64; 4] {
    [Xv6Status::InvalidArgument.raw(), 0, 0, 0]
}

pub(crate) fn send_host_async_reply(request_id: u64, reply: [u64; 4]) {
    unsafe {
        sel4_send(
            XV6_HOST_REPLY_ENDPOINT_CPTR,
            msg_info(Xv6Protocol::HostToVfsAsync.raw(), 0, 0, 5),
            &[request_id, reply[0], reply[1], reply[2], reply[3]],
        );
    }
}

pub(crate) fn reply4(reply: &IpcMessage) -> [u64; 4] {
    [reply.mrs[0], reply.mrs[1], reply.mrs[2], reply.mrs[3]]
}

pub(crate) fn xv6fs_retain(inum: u32) -> bool {
    let Some(reply) = xv6fs_call(
        Xv6FsOp::Retain.raw(),
        &[Xv6Protocol::VfsToXv6Fs.raw(), XV6_ABI_VERSION, inum as u64],
    ) else {
        return false;
    };
    reply.mrs[0] == Xv6Status::Ok.raw()
}

pub(crate) fn xv6fs_release(inum: u32) -> bool {
    let Some(reply) = xv6fs_call(
        Xv6FsOp::Release.raw(),
        &[Xv6Protocol::VfsToXv6Fs.raw(), XV6_ABI_VERSION, inum as u64],
    ) else {
        return false;
    };
    reply.mrs[0] == Xv6Status::Ok.raw()
}

pub(crate) async fn xv6fs_retain_async(inum: u32) -> bool {
    let Some(reply) = xv6fs_call_async(
        Xv6FsOp::Retain.raw(),
        &[Xv6Protocol::VfsToXv6Fs.raw(), XV6_ABI_VERSION, inum as u64],
    )
    .await
    else {
        return false;
    };
    reply[0] == Xv6Status::Ok.raw()
}

pub(crate) async fn xv6fs_release_async(inum: u32) -> bool {
    let Some(reply) = xv6fs_call_async(
        Xv6FsOp::Release.raw(),
        &[Xv6Protocol::VfsToXv6Fs.raw(), XV6_ABI_VERSION, inum as u64],
    )
    .await
    else {
        return false;
    };
    reply[0] == Xv6Status::Ok.raw()
}

pub(crate) fn xv6fs_call(label: u64, mrs: &[u64]) -> Option<IpcMessage> {
    let info = msg_info(label, 0, 0, mrs.len() as u64);
    let mut retries = 0usize;
    loop {
        let reply = unsafe { sel4_call(XV6_XV6FS_ENDPOINT_CPTR, info, mrs) };
        if msg_label(reply.info) != 0 {
            return None;
        }
        if reply.mrs[0] != Xv6Status::Busy.raw() {
            return Some(reply);
        }
        retries += 1;
        if retries >= FS_BUSY_RETRY_LIMIT {
            warn!("vfs-server: xv6fs busy retry exhausted op={}", label);
            return None;
        }
        unsafe {
            sel4_yield();
        }
    }
}

pub(crate) async fn xv6fs_call_async(label: u64, mrs: &[u64]) -> Option<[u64; 4]> {
    xv6fs_call(label, mrs).map(|reply| reply4(&reply))
}

pub(crate) fn path_mrs_valid(msg: &IpcMessage, start: usize, path_len: usize) -> bool {
    path_len > 0
        && path_len <= MAX_PATH_BYTES
        && start + path_len.div_ceil(8) <= msg_len(msg.info) as usize
}

pub(crate) fn copy_path_words(
    msg: &IpcMessage,
    src_start: usize,
    path_len: usize,
    out: &mut [u64],
    dst: usize,
) {
    let words = path_len.div_ceil(8);
    let mut i = 0usize;
    while i < words {
        out[dst + i] = msg.mrs[src_start + i];
        i += 1;
    }
}

pub(crate) fn with_shared_buffer<R>(op: impl FnOnce(&[u8]) -> R) -> R {
    let buffer = unsafe {
        core::slice::from_raw_parts(
            xv6_abi::XV6_DISK_SHARED_BUFFER_VADDR as *const u8,
            XV6_MAX_FILE_WRITE,
        )
    };
    op(buffer)
}

pub(crate) fn with_shared_buffer_mut<R>(op: impl FnOnce(&mut [u8]) -> R) -> R {
    let buffer = unsafe {
        core::slice::from_raw_parts_mut(
            xv6_abi::XV6_DISK_SHARED_BUFFER_VADDR as *mut u8,
            XV6_MAX_FILE_WRITE,
        )
    };
    op(buffer)
}
