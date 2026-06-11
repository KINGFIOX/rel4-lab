use core::cell::UnsafeCell;

use xv6_abi::{MAX_OPEN_FILES, MAX_PIPES, PIPE_BUF};

pub(crate) const CONSOLE_BUF_SIZE: usize = 128;

pub(crate) const FILE_CLOSED: u8 = 0;
pub(crate) const FILE_XV6_FILE: u8 = 1;
pub(crate) const FILE_XV6_DIR: u8 = 2;
pub(crate) const FILE_PIPE_READ: u8 = 3;
pub(crate) const FILE_PIPE_WRITE: u8 = 4;
pub(crate) const FILE_CONSOLE: u8 = 5;

#[derive(Copy, Clone)]
pub(crate) struct OpenFile {
    pub(crate) used: bool,
    pub(crate) refs: u16,
    pub(crate) kind: u8,
    pub(crate) node: u32,
    pub(crate) aux: usize,
    pub(crate) offset: usize,
    pub(crate) readable: bool,
    pub(crate) writable: bool,
    pub(crate) busy: bool,
}

impl OpenFile {
    const fn closed() -> Self {
        Self {
            used: false,
            refs: 0,
            kind: FILE_CLOSED,
            node: 0,
            aux: 0,
            offset: 0,
            readable: false,
            writable: false,
            busy: false,
        }
    }
}

#[derive(Copy, Clone)]
pub(crate) struct Pipe {
    pub(crate) buf: [u8; PIPE_BUF],
    pub(crate) read_pos: usize,
    pub(crate) len: usize,
    pub(crate) readers: usize,
    pub(crate) writers: usize,
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

pub(crate) struct ConsoleState {
    pub(crate) buf: [u8; CONSOLE_BUF_SIZE],
    pub(crate) r: usize,
    pub(crate) w: usize,
    pub(crate) e: usize,
    pub(crate) input_pos: usize,
}

impl ConsoleState {
    const fn new() -> Self {
        Self {
            buf: [0; CONSOLE_BUF_SIZE],
            r: 0,
            w: 0,
            e: 0,
            input_pos: 0,
        }
    }
}

struct VfsRuntimeState {
    files: [OpenFile; MAX_OPEN_FILES],
    pipes: [Pipe; MAX_PIPES],
    console: ConsoleState,
}

impl VfsRuntimeState {
    const fn new() -> Self {
        Self {
            files: [OpenFile::closed(); MAX_OPEN_FILES],
            pipes: [Pipe::closed(); MAX_PIPES],
            console: ConsoleState::new(),
        }
    }
}

struct VfsRuntime {
    state: UnsafeCell<VfsRuntimeState>,
}

// vfs-server runs one cooperative request loop. Runtime tables are only
// mutated through this module's scoped accessors.
unsafe impl Sync for VfsRuntime {}

impl VfsRuntime {
    const fn new() -> Self {
        Self {
            state: UnsafeCell::new(VfsRuntimeState::new()),
        }
    }

    fn state(&self) -> &VfsRuntimeState {
        unsafe { &*self.state.get() }
    }

    fn state_mut(&self) -> &mut VfsRuntimeState {
        unsafe { &mut *self.state.get() }
    }
}

static VFS_RUNTIME: VfsRuntime = VfsRuntime::new();

pub(crate) enum ReleaseResult {
    Invalid,
    Done,
    Xv6(u32),
}

pub(crate) fn reset_all() {
    let state = VFS_RUNTIME.state_mut();
    let mut i = 0usize;
    while i < MAX_OPEN_FILES {
        state.files[i] = OpenFile::closed();
        i += 1;
    }
    i = 0;
    while i < MAX_PIPES {
        state.pipes[i] = Pipe::closed();
        i += 1;
    }
    state.console = ConsoleState::new();
}

pub(crate) fn valid_file(file: usize) -> Option<usize> {
    if file < MAX_OPEN_FILES && VFS_RUNTIME.state().files[file].used {
        Some(file)
    } else {
        None
    }
}

pub(crate) fn file_snapshot(file: usize) -> Option<OpenFile> {
    valid_file(file).map(|file| VFS_RUNTIME.state().files[file])
}

pub(crate) fn alloc_file(
    kind: u8,
    node: u32,
    aux: usize,
    readable: bool,
    writable: bool,
) -> Option<usize> {
    let state = VFS_RUNTIME.state_mut();
    let mut i = 0usize;
    while i < MAX_OPEN_FILES {
        if !state.files[i].used {
            state.files[i] = OpenFile {
                used: true,
                refs: 1,
                kind,
                node,
                aux,
                offset: 0,
                readable,
                writable,
                busy: false,
            };
            return Some(i);
        }
        i += 1;
    }
    None
}

pub(crate) fn retain_file(file: usize) -> bool {
    let state = VFS_RUNTIME.state_mut();
    if file >= MAX_OPEN_FILES || !state.files[file].used || state.files[file].refs == u16::MAX {
        return false;
    }
    state.files[file].refs += 1;
    true
}

pub(crate) fn release_file(file: usize) -> bool {
    let state = VFS_RUNTIME.state();
    if file < MAX_OPEN_FILES
        && state.files[file].used
        && (state.files[file].kind == FILE_XV6_FILE || state.files[file].kind == FILE_XV6_DIR)
    {
        return false;
    }
    match detach_file(file) {
        ReleaseResult::Invalid => false,
        ReleaseResult::Done => true,
        ReleaseResult::Xv6(_) => false,
    }
}

pub(crate) fn detach_file(file: usize) -> ReleaseResult {
    let state = VFS_RUNTIME.state_mut();
    if file >= MAX_OPEN_FILES || !state.files[file].used {
        return ReleaseResult::Invalid;
    }
    if state.files[file].refs > 1 {
        state.files[file].refs -= 1;
        return ReleaseResult::Done;
    }
    let old = state.files[file];
    state.files[file] = OpenFile::closed();
    match old.kind {
        FILE_XV6_FILE | FILE_XV6_DIR => return ReleaseResult::Xv6(old.node),
        FILE_PIPE_READ => {
            if old.aux < MAX_PIPES && state.pipes[old.aux].readers > 0 {
                state.pipes[old.aux].readers -= 1;
            }
        }
        FILE_PIPE_WRITE => {
            if old.aux < MAX_PIPES && state.pipes[old.aux].writers > 0 {
                state.pipes[old.aux].writers -= 1;
            }
        }
        _ => {}
    }
    ReleaseResult::Done
}

pub(crate) fn alloc_pipe() -> Option<usize> {
    let state = VFS_RUNTIME.state_mut();
    let mut i = 0usize;
    while i < MAX_PIPES {
        if state.pipes[i].readers == 0 && state.pipes[i].writers == 0 {
            state.pipes[i] = Pipe::closed();
            return Some(i);
        }
        i += 1;
    }
    None
}

pub(crate) fn clear_pipe(pipe_idx: usize) {
    if pipe_idx < MAX_PIPES {
        VFS_RUNTIME.state_mut().pipes[pipe_idx] = Pipe::closed();
    }
}

pub(crate) fn open_pipe(pipe_idx: usize) -> bool {
    if pipe_idx >= MAX_PIPES {
        return false;
    }
    let pipe = &mut VFS_RUNTIME.state_mut().pipes[pipe_idx];
    pipe.readers = 1;
    pipe.writers = 1;
    true
}

pub(crate) fn with_pipe_mut<R>(pipe_idx: usize, op: impl FnOnce(&mut Pipe) -> R) -> Option<R> {
    if pipe_idx >= MAX_PIPES {
        return None;
    }
    Some(op(&mut VFS_RUNTIME.state_mut().pipes[pipe_idx]))
}

pub(crate) fn add_file_offset(file: usize, amount: usize) {
    if file < MAX_OPEN_FILES {
        let file = &mut VFS_RUNTIME.state_mut().files[file];
        file.offset = file.offset.saturating_add(amount);
    }
}

pub(crate) fn acquire_file_io(file: usize) -> bool {
    if file >= MAX_OPEN_FILES {
        return false;
    }
    let file = &mut VFS_RUNTIME.state_mut().files[file];
    if !file.used || file.busy {
        return false;
    }
    file.busy = true;
    true
}

pub(crate) fn release_file_io(file: usize) {
    if file < MAX_OPEN_FILES {
        VFS_RUNTIME.state_mut().files[file].busy = false;
    }
}

pub(crate) fn with_console<R>(op: impl FnOnce(&ConsoleState) -> R) -> R {
    op(&VFS_RUNTIME.state().console)
}

pub(crate) fn with_console_mut<R>(op: impl FnOnce(&mut ConsoleState) -> R) -> R {
    op(&mut VFS_RUNTIME.state_mut().console)
}
