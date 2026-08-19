use core::cell::UnsafeCell;
use core::ptr;

use crate::allocator::Allocator;
use crate::arch::current as arch;
use crate::child::{ensure_host_page_table_path, write_user_context};
use crate::consts::*;
use crate::util::{halt_loop, info, warn};
use sel4_user::{
    ThreadCtl, call_checked, cap_rights, cnode_cap_data, enable_tls_ipc, install_thread_ctl, rt,
};

pub(crate) const NUM_WORKER_THREADS: usize = 1;

const WORKER_REGION_BASE: u64 = 0x6000_0000;
const WORKER_STACK_PAGES: usize = 16;
const WORKER_PAGES: usize = 1 + WORKER_STACK_PAGES + 1 + 1;

struct MainCtl {
    ctl: UnsafeCell<ThreadCtl>,
}

unsafe impl Sync for MainCtl {}

static MAIN_CTL: MainCtl = MainCtl {
    ctl: UnsafeCell::new(ThreadCtl {
        self_ptr: ptr::null_mut(),
        ipc_buffer: ptr::null_mut(),
    }),
};

pub(crate) fn install_main_tls(ipc_buffer: u64) {
    if NUM_WORKER_THREADS == 0 {
        return;
    }
    let ctl = MAIN_CTL.ctl.get();
    unsafe {
        (*ctl).self_ptr = ctl;
        (*ctl).ipc_buffer = ipc_buffer as *mut sel4_user::IpcBuffer;
        install_thread_ctl(ctl);
    }
    enable_tls_ipc();
}

pub(crate) fn start_workers(alloc: &mut Allocator, _wake_ntfn: u64, cnode_size_bits: u64) {
    if NUM_WORKER_THREADS == 0 {
        return;
    }
    if cnode_size_bits == 0 || cnode_size_bits > WORD_BITS {
        warn!(
            "linux-compat: invalid root CNode size bits={}",
            cnode_size_bits
        );
        halt_loop();
    }
    let gp = current_gp();
    let entry = rt::worker_entry as *const () as u64;
    let mut i = 0usize;
    while i < NUM_WORKER_THREADS {
        start_one_worker(alloc, i, entry, gp, cnode_size_bits);
        i += 1;
    }
    info!(
        "linux-compat: started {} worker thread(s)",
        NUM_WORKER_THREADS
    );
}

fn start_one_worker(
    alloc: &mut Allocator,
    index: usize,
    entry: u64,
    gp: u64,
    cnode_size_bits: u64,
) {
    let base = WORKER_REGION_BASE + index as u64 * (WORKER_PAGES as u64 * PAGE_SIZE);
    let stack_base = base + PAGE_SIZE;
    let stack_top = stack_base + WORKER_STACK_PAGES as u64 * PAGE_SIZE;
    let ipc_va = stack_top;
    let ctl_va = ipc_va + PAGE_SIZE;

    let mut page = 0usize;
    while page < WORKER_STACK_PAGES {
        let frame = alloc.retype_one(OBJ_4K, 0);
        map_host_frame(alloc, frame, stack_base + page as u64 * PAGE_SIZE);
        page += 1;
    }
    let ipc_frame = alloc.retype_one(OBJ_4K, 0);
    map_host_frame(alloc, ipc_frame, ipc_va);
    let ctl_frame = alloc.retype_one(OBJ_4K, 0);
    map_host_frame(alloc, ctl_frame, ctl_va);

    let ctl = ctl_va as *mut ThreadCtl;
    unsafe {
        ptr::write_bytes(ipc_va as *mut u8, 0, PAGE_SIZE as usize);
        ptr::write_bytes(ctl_va as *mut u8, 0, PAGE_SIZE as usize);
        (*ctl).self_ptr = ctl;
        (*ctl).ipc_buffer = ipc_va as *mut sel4_user::IpcBuffer;
    }

    let tcb = alloc.retype_one(OBJ_TCB, 0);
    let cspace_data = cnode_cap_data(0, WORD_BITS - cnode_size_bits);
    call_checked(
        tcb,
        LABEL_TCB_CONFIGURE,
        &[ROOT_CNODE, INIT_VSPACE, ipc_frame],
        &[0, cspace_data, 0, ipc_va],
    );
    call_checked(
        tcb,
        LABEL_TCB_SET_SCHED_PARAMS,
        &[INIT_TCB],
        &[CHILD_MCP, CHILD_PRIORITY],
    );
    call_checked(tcb, LABEL_TCB_SET_TLS_BASE, &[], &[ctl_va]);
    let ctx = arch::new_worker_context(entry, stack_top, ctl_va, gp);
    write_user_context(tcb, &ctx, true);
}

fn map_host_frame(alloc: &mut Allocator, frame_slot: u64, va: u64) {
    ensure_host_page_table_path(alloc, va);
    call_checked(
        frame_slot,
        LABEL_PAGE_MAP,
        &[INIT_VSPACE],
        &[va, cap_rights(false, false, true, true), 1],
    );
}

fn current_gp() -> u64 {
    #[cfg(target_arch = "riscv64")]
    unsafe {
        let gp: u64;
        core::arch::asm!(
            "mv {}, gp",
            out(reg) gp,
            options(nomem, nostack, preserves_flags)
        );
        gp
    }
    #[cfg(not(target_arch = "riscv64"))]
    {
        0
    }
}
