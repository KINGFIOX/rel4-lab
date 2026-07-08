use crate::object::tcb::Tcb;

pub fn init_current_core() {}

pub fn clear_supervisor_access() {}

pub fn disable_access() {}

pub fn lazy_restore(_thread: *mut Tcb) {}

pub fn release(_thread: *mut Tcb) {}

pub fn release_on_current_core(_thread: *mut Tcb) {}
