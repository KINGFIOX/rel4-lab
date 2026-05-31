use core::cmp::min;

use crate::allocator::Allocator;
use crate::child::copy_to_child;
use crate::consts::{MAX_PIPES, PIPE_BUF};
use crate::types::Child;

#[derive(Copy, Clone)]
struct Pipe {
    buf: [u8; PIPE_BUF],
    read_pos: usize,
    len: usize,
    readers: usize,
    writers: usize,
}

impl Pipe {
    const fn closed() -> Self {
        Self {
            buf: [0; PIPE_BUF],
            read_pos: 0,
            len: 0,
            readers: 0,
            writers: 0,
        }
    }
}

static mut PIPES: [Pipe; MAX_PIPES] = [Pipe::closed(); MAX_PIPES];

pub(crate) fn reset_all() {
    unsafe {
        let mut i = 0;
        while i < MAX_PIPES {
            PIPES[i] = Pipe::closed();
            i += 1;
        }
    }
}

pub(crate) fn alloc_unused() -> Option<usize> {
    unsafe {
        for i in 0..MAX_PIPES {
            if PIPES[i].readers == 0 && PIPES[i].writers == 0 {
                return Some(i);
            }
        }
    }
    None
}

pub(crate) fn init_pair(pipe_idx: usize) -> bool {
    unsafe {
        if pipe_idx >= MAX_PIPES {
            return false;
        }
        PIPES[pipe_idx] = Pipe::closed();
        PIPES[pipe_idx].readers = 1;
        PIPES[pipe_idx].writers = 1;
        true
    }
}

pub(crate) fn add_reader(pipe_idx: usize) {
    unsafe {
        if pipe_idx < MAX_PIPES {
            PIPES[pipe_idx].readers += 1;
        }
    }
}

pub(crate) fn add_writer(pipe_idx: usize) {
    unsafe {
        if pipe_idx < MAX_PIPES {
            PIPES[pipe_idx].writers += 1;
        }
    }
}

pub(crate) fn close_reader(pipe_idx: usize) {
    unsafe {
        if pipe_idx < MAX_PIPES && PIPES[pipe_idx].readers > 0 {
            PIPES[pipe_idx].readers -= 1;
        }
    }
}

pub(crate) fn close_writer(pipe_idx: usize) {
    unsafe {
        if pipe_idx < MAX_PIPES && PIPES[pipe_idx].writers > 0 {
            PIPES[pipe_idx].writers -= 1;
        }
    }
}

pub(crate) fn has_readers(pipe_idx: usize) -> bool {
    unsafe { pipe_idx < MAX_PIPES && PIPES[pipe_idx].readers > 0 }
}

pub(crate) fn has_writers(pipe_idx: usize) -> bool {
    unsafe { pipe_idx < MAX_PIPES && PIPES[pipe_idx].writers > 0 }
}

pub(crate) fn write(pipe_idx: usize, src: &[u8]) -> usize {
    unsafe {
        if pipe_idx >= MAX_PIPES || PIPES[pipe_idx].readers == 0 {
            return 0;
        }
        let pipe = &mut PIPES[pipe_idx];
        let mut n = 0;
        while n < src.len() && pipe.len < PIPE_BUF {
            let write_pos = (pipe.read_pos + pipe.len) % PIPE_BUF;
            pipe.buf[write_pos] = src[n];
            pipe.len += 1;
            n += 1;
        }
        n
    }
}

pub(crate) fn read(
    alloc: &mut Allocator,
    child: &Child,
    pipe_idx: usize,
    dst: u64,
    len: usize,
) -> i64 {
    unsafe {
        if pipe_idx >= MAX_PIPES {
            return -1;
        }
        let pipe = &mut PIPES[pipe_idx];
        if pipe.len == 0 {
            return 0;
        }

        let mut total = 0usize;
        let mut scratch = [0u8; 128];
        while total < len && pipe.len > 0 {
            let n = min(scratch.len(), min(len - total, pipe.len));
            let mut i = 0;
            while i < n {
                scratch[i] = pipe.buf[pipe.read_pos];
                pipe.read_pos = (pipe.read_pos + 1) % PIPE_BUF;
                pipe.len -= 1;
                i += 1;
            }
            if !copy_to_child(alloc, child, dst + total as u64, &scratch[..n]) {
                return -1;
            }
            total += n;
        }
        total as i64
    }
}
