use linux_abi::{
    EINVAL, HOST_REPLY_ENDPOINT_CPTR, IpcProtocol, IpcStatus, LINUX_ABI_VERSION, MAX_IO_BYTES,
    MAX_PATH_BYTES, SHARED_BUFFER_VADDR,
};
use sel4_user::{IpcMessage, msg_info, msg_len, sel4_send};

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
    if msg.mrs[0] == IpcProtocol::HostToVfs.raw() && msg.mrs[1] == LINUX_ABI_VERSION {
        Some(HostRequest::sync())
    } else if msg.mrs[0] == IpcProtocol::HostToVfsAsync.raw() && msg.mrs[1] != 0 {
        Some(HostRequest {
            async_request: true,
            request_id: msg.mrs[1],
        })
    } else {
        None
    }
}

pub(crate) fn ok() -> [u64; 4] {
    [IpcStatus::Ok.raw(), 0, 0, 0]
}

pub(crate) fn err() -> [u64; 4] {
    err_code(EINVAL)
}

pub(crate) fn err_code(errno: i32) -> [u64; 4] {
    [errno as u64, 0, 0, 0]
}

pub(crate) fn send_host_async_reply(request_id: u64, reply: [u64; 4]) {
    unsafe {
        sel4_send(
            HOST_REPLY_ENDPOINT_CPTR,
            msg_info(IpcProtocol::HostToVfsAsync.raw(), 0, 0, 5),
            &[request_id, reply[0], reply[1], reply[2], reply[3]],
        );
    }
}

pub(crate) fn path_mrs_valid(msg: &IpcMessage, start: usize, path_len: usize) -> bool {
    path_len > 0
        && path_len <= MAX_PATH_BYTES
        && start + path_len.div_ceil(8) <= msg_len(msg.info) as usize
}

pub(crate) fn with_shared_buffer<R>(op: impl FnOnce(&[u8]) -> R) -> R {
    let buffer =
        unsafe { core::slice::from_raw_parts(SHARED_BUFFER_VADDR as *const u8, MAX_IO_BYTES) };
    op(buffer)
}

pub(crate) fn with_shared_buffer_mut<R>(op: impl FnOnce(&mut [u8]) -> R) -> R {
    let buffer =
        unsafe { core::slice::from_raw_parts_mut(SHARED_BUFFER_VADDR as *mut u8, MAX_IO_BYTES) };
    op(buffer)
}
