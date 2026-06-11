use core::cmp::min;
use core::sync::atomic::{Ordering, fence};

use sel4_user::IpcMessage;
use xv6_abi::{PIPE_BUF, Xv6FileType, Xv6Status};

use crate::ipc::{err, valid_host, with_shared_buffer, with_shared_buffer_mut};
use crate::state::{
    FILE_PIPE_READ, FILE_PIPE_WRITE, alloc_file, alloc_pipe, clear_pipe, open_pipe, release_file,
    with_pipe_mut,
};

pub(crate) fn handle_pipe(msg: &IpcMessage) -> [u64; 4] {
    if !valid_host(msg) {
        return err();
    }
    let Some(pipe_idx) = alloc_pipe() else {
        return err();
    };
    let Some(read_file) = alloc_file(FILE_PIPE_READ, 0, pipe_idx, true, false) else {
        clear_pipe(pipe_idx);
        return err();
    };
    let Some(write_file) = alloc_file(FILE_PIPE_WRITE, 0, pipe_idx, false, true) else {
        release_file(read_file);
        clear_pipe(pipe_idx);
        return err();
    };
    if !open_pipe(pipe_idx) {
        release_file(read_file);
        release_file(write_file);
        clear_pipe(pipe_idx);
        return err();
    }
    [Xv6Status::Ok.raw(), read_file as u64, write_file as u64, 0]
}

pub(crate) fn read_pipe(pipe_idx: usize, max_len: usize) -> [u64; 4] {
    with_pipe_mut(pipe_idx, |pipe| {
        if pipe.len == 0 {
            if pipe.writers > 0 {
                return [Xv6Status::WouldBlock.raw(), 0, 0, 0];
            }
            return [Xv6Status::Ok.raw(), 0, Xv6FileType::File.raw() as u64, 0];
        }
        let n = min(max_len, pipe.len);
        with_shared_buffer_mut(|dst| {
            let mut i = 0usize;
            while i < n {
                dst[i] = pipe.buf[pipe.read_pos];
                pipe.read_pos = (pipe.read_pos + 1) % PIPE_BUF;
                pipe.len -= 1;
                i += 1;
            }
        });
        fence(Ordering::SeqCst);
        [
            Xv6Status::Ok.raw(),
            n as u64,
            Xv6FileType::File.raw() as u64,
            0,
        ]
    })
    .unwrap_or_else(err)
}

pub(crate) fn write_pipe(pipe_idx: usize, max_len: usize) -> [u64; 4] {
    with_pipe_mut(pipe_idx, |pipe| {
        if pipe.readers == 0 {
            return [Xv6Status::BrokenPipe.raw(), 0, 0, 0];
        }
        if pipe.len >= PIPE_BUF {
            return [Xv6Status::WouldBlock.raw(), 0, 0, 0];
        }
        let n = with_shared_buffer(|src| {
            let mut n = 0usize;
            while n < max_len && pipe.len < PIPE_BUF {
                let write_pos = (pipe.read_pos + pipe.len) % PIPE_BUF;
                pipe.buf[write_pos] = src[n];
                pipe.len += 1;
                n += 1;
            }
            n
        });
        [
            Xv6Status::Ok.raw(),
            n as u64,
            Xv6FileType::File.raw() as u64,
            0,
        ]
    })
    .unwrap_or_else(err)
}
