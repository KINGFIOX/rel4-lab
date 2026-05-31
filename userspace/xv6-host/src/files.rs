use crate::consts::MAX_OPEN_FILES;
use crate::types::FdEntry;

#[derive(Copy, Clone)]
struct OpenFile {
    used: bool,
    refs: u16,
    offset: usize,
}

impl OpenFile {
    const fn closed() -> Self {
        Self {
            used: false,
            refs: 0,
            offset: 0,
        }
    }
}

static mut OPEN_FILES: [OpenFile; MAX_OPEN_FILES] = [OpenFile::closed(); MAX_OPEN_FILES];

pub(crate) fn reset_all() {
    unsafe {
        let mut i = 0;
        while i < MAX_OPEN_FILES {
            OPEN_FILES[i] = OpenFile::closed();
            i += 1;
        }
    }
}

pub(crate) fn alloc() -> Option<usize> {
    unsafe {
        let mut i = 0;
        while i < MAX_OPEN_FILES {
            if !OPEN_FILES[i].used {
                OPEN_FILES[i] = OpenFile {
                    used: true,
                    refs: 1,
                    offset: 0,
                };
                return Some(i);
            }
            i += 1;
        }
    }
    None
}

pub(crate) fn retain(entry: FdEntry) {
    unsafe {
        if entry.file < MAX_OPEN_FILES && OPEN_FILES[entry.file].used {
            OPEN_FILES[entry.file].refs = OPEN_FILES[entry.file].refs.saturating_add(1);
        }
    }
}

pub(crate) fn release(entry: FdEntry) -> bool {
    unsafe {
        if entry.file >= MAX_OPEN_FILES || !OPEN_FILES[entry.file].used {
            return false;
        }
        if OPEN_FILES[entry.file].refs > 1 {
            OPEN_FILES[entry.file].refs -= 1;
            return false;
        }
        OPEN_FILES[entry.file] = OpenFile::closed();
        true
    }
}

pub(crate) fn force_close(file: usize) {
    unsafe {
        if file < MAX_OPEN_FILES {
            OPEN_FILES[file] = OpenFile::closed();
        }
    }
}

pub(crate) fn offset(entry: FdEntry) -> Option<usize> {
    unsafe {
        if entry.file < MAX_OPEN_FILES && OPEN_FILES[entry.file].used {
            Some(OPEN_FILES[entry.file].offset)
        } else {
            None
        }
    }
}

pub(crate) fn advance_offset(entry: FdEntry, by: usize) {
    unsafe {
        if entry.file < MAX_OPEN_FILES && OPEN_FILES[entry.file].used {
            OPEN_FILES[entry.file].offset = OPEN_FILES[entry.file].offset.saturating_add(by);
        }
    }
}
