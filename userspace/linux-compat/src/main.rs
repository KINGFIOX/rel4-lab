#![no_std]
#![no_main]
#![deny(unsafe_attr_outside_unsafe)]
#![deny(unsafe_op_in_unsafe_fn)]

use core::cell::UnsafeCell;
use core::panic::PanicInfo;
use core::ptr;
use core::sync::atomic::{AtomicBool, Ordering};

mod allocator;
mod arch;
mod child;
mod consts;
mod exec_syscalls;
mod fs_syscalls;
mod io_syscalls;
mod linux;
mod memory_syscalls;
mod process_syscalls;
mod reply_caps;
mod types;
mod util;
mod vfs;

use crate::arch::current as host_arch;
use allocator::Allocator;
use child::{
    create_child, create_child_from_untyped, ensure_host_page_table_path, load_elf,
    map_existing_child_frame_with_attrs, map_stack_pages, mint_cap_to_child,
    start_child_with_a0_a1,
};
use consts::{
    FAULT_UNKNOWN_SYSCALL, FAULT_VM_FAULT, HOST_REPLY_ENDPOINT_CPTR, INIT_PATH, IpcBadge,
    IpcProtocol, IpcStatus, LABEL_IRQ_ISSUE_IRQ_HANDLER, LABEL_IRQ_SET_NOTIFICATION,
    LABEL_PAGE_MAP, LINUX_ABI_VERSION, SERVICE_ENDPOINT_CPTR, SHARED_BUFFER_PAGES,
    SHARED_BUFFER_VADDR, UART_ENDPOINT_CPTR, UART_MMIO_FRAME_VADDR, UART_REPLY_ENDPOINT_CPTR,
    UART_SERVER_ELF, UART_SERVER_PID, UART0_MMIO_FRAME_BASE, VFS_SERVER_ELF, VFS_SERVER_PID,
    VFS_SERVICE_UNTYPED_BITS, VfsOp,
};
use consts::{
    INIT_TCB, INIT_VSPACE, IRQ_CONTROL, KERNEL_TIMER_IRQ, MAX_PROCS, OBJ_4K, OBJ_ENDPOINT,
    OBJ_NOTIFICATION, OBJ_REPLY, OBJ_UNTYPED,
};
use consts::{LABEL_TCB_BIND_NOTIFICATION, PAGE_SIZE, ROOT_CNODE_DEPTH};
use consts::{
    PROC_RUNNABLE, PROC_UNUSED, PROC_VFS_DEFERRED, ROOT_CNODE, SERVER_CNODE_CPTR,
    SERVER_RECV_REPLY_CPTR, SERVICE_UNTYPED_BITS, VM_ATTR_UNCACHED,
};
use exec_syscalls::load_init_program;
use linux::{handle_linux_fault, handle_linux_syscall};
use sel4_user::{
    call_checked, cap_rights, cnode_cap_data, init_ipc_buffer, msg_info, msg_label, sel4_call,
    sel4_recv_with_reply, sel4_reply_recv_with_reply,
};
use types::{BootInfo, SyscallResult, TaskStruct};
use util::{error, halt_loop, info, init_logger, warn};

static SAW_FAULT_IPC: AtomicBool = AtomicBool::new(false);

struct ProcessTable {
    procs: UnsafeCell<[TaskStruct; MAX_PROCS]>,
}

// linux-compat mutates the process table from the single rootserver fault loop.
unsafe impl Sync for ProcessTable {}

impl ProcessTable {
    const fn new() -> Self {
        Self {
            procs: UnsafeCell::new([TaskStruct::empty(); MAX_PROCS]),
        }
    }

    fn procs(&self) -> &mut [TaskStruct; MAX_PROCS] {
        unsafe { &mut *self.procs.get() }
    }
}

static PROCESS_TABLE: ProcessTable = ProcessTable::new();

#[unsafe(no_mangle)]
pub extern "C" fn _start(bootinfo: usize) -> ! {
    unsafe {
        clear_bss();
    }
    run(bootinfo as *const BootInfo);
}

unsafe fn clear_bss() {
    unsafe extern "C" {
        static __bss_start: u8;
        static __bss_end: u8;
    }
    unsafe {
        let start = core::ptr::addr_of!(__bss_start) as usize;
        let end = core::ptr::addr_of!(__bss_end) as usize;
        ptr::write_bytes(start as *mut u8, 0, end.saturating_sub(start));
    }
}

fn run(bi_ptr: *const BootInfo) -> ! {
    let bi = unsafe { &*bi_ptr };
    init_ipc_buffer(bi.ipc_buffer);
    init_logger();
    info!("linux-compat: boot");

    let mut alloc = Allocator::new(bi);
    let fault_ep = alloc.retype_one(OBJ_ENDPOINT, 0);
    let procs = PROCESS_TABLE.procs();
    let vfs_ep = spawn_service_servers(&mut alloc, fault_ep);
    init_vfs_server(vfs_ep);
    linux::init_vfs_client(vfs_ep);
    procs[0] = create_child(&mut alloc, 0, 1, 0, fault_ep);
    linux::init_vfs_process(&mut procs[0]);
    setup_timer_notification(&mut alloc);
    reply_caps::init(&mut alloc);
    load_init_program(&mut alloc, &mut procs[0], INIT_PATH);

    info!("linux-compat: waiting for fault IPC");
    let mut pending_reply: Option<(u64, [u64; host_arch::FAULT_REPLY_WORDS])> = None;
    loop {
        let msg = if let Some((reply_info, reply_mrs)) = pending_reply.take() {
            let reply_slot = reply_caps::take_current();
            let msg =
                unsafe { sel4_reply_recv_with_reply(fault_ep, reply_info, &reply_mrs, reply_slot) };
            reply_caps::set_current(reply_slot);
            msg
        } else {
            let reply_slot = reply_caps::acquire();
            let msg = unsafe { sel4_recv_with_reply(fault_ep, reply_slot) };
            reply_caps::set_current(reply_slot);
            msg
        };

        if (msg.badge & IpcBadge::VfsReply.raw()) != 0 {
            if let Some(pump_waiters) = linux::complete_vfs_async_reply(&mut alloc, procs, &msg) {
                reply_caps::release_current();
                if pump_waiters {
                    linux::pump_vfs_waiters(&mut alloc, procs);
                }
                linux::pump_sleep_waiters(procs);
                pump_deferred_syscalls(&mut alloc, procs);
                continue;
            }
        }

        let label = msg_label(msg.info);
        if label == 0 {
            reply_caps::release_current();
            linux::tick();
            linux::pump_vfs_waiters(&mut alloc, procs);
            linux::pump_sleep_waiters(procs);
            continue;
        }
        let Some(proc_idx) = find_proc_by_pid(procs, msg.badge) else {
            warn!("linux-compat: fault from unknown pid={}", msg.badge);
            halt_loop();
        };

        if label == FAULT_UNKNOWN_SYSCALL
            && linux::has_active_vfs_async_requests()
            && linux::should_defer_vfs_syscall(&msg.mrs)
        {
            defer_vfs_syscall(&mut procs[proc_idx], &msg.mrs);
            continue;
        }

        let result = if label == FAULT_UNKNOWN_SYSCALL {
            if !SAW_FAULT_IPC.swap(true, Ordering::Relaxed) {
                info!("linux-compat: UnknownSyscall fault IPC");
            }
            handle_linux_syscall(&mut alloc, procs, proc_idx, &msg.mrs)
        } else {
            if label != FAULT_VM_FAULT {
                warn!("linux-compat: non-syscall fault label={}", label);
            }
            handle_linux_fault(&mut alloc, procs, proc_idx, label, &msg.mrs)
        };
        linux::pump_vfs_waiters(&mut alloc, procs);
        linux::pump_sleep_waiters(procs);

        match result {
            SyscallResult::Reply(ret) => {
                let mut reply_mrs = host_arch::syscall_reply_frame(&msg.mrs);
                host_arch::set_syscall_return_value(&mut reply_mrs, ret as u64);
                pending_reply = Some((
                    msg_info(0, 0, 0, host_arch::FAULT_REPLY_WORDS as u64),
                    reply_mrs,
                ));
            }
            SyscallResult::ReplyFrame(frame) => {
                pending_reply = Some((
                    msg_info(0, 0, 0, host_arch::FAULT_REPLY_WORDS as u64),
                    frame,
                ));
            }
            SyscallResult::Block => {
                if reply_caps::has_current() {
                    warn!("linux-compat: blocked syscall did not save its reply cap");
                    halt_loop();
                }
            }
            SyscallResult::Stop => {
                let reply_slot = reply_caps::take_current();
                reply_caps::stop_and_release(reply_slot);
            }
        }
        pump_deferred_syscalls(&mut alloc, procs);
    }
}

fn defer_vfs_syscall(child: &mut TaskStruct, mrs: &[u64; 64]) {
    let reply_slot = reply_caps::take_current();
    child.deferred_reply_slot = reply_slot;
    child.deferred_mrs = *mrs;
    child.state = PROC_VFS_DEFERRED;
}

fn pump_deferred_syscalls(alloc: &mut Allocator, procs: &mut [TaskStruct; MAX_PROCS]) {
    if linux::has_active_vfs_async_requests() {
        return;
    }
    let mut i = 0usize;
    while i < MAX_PROCS {
        if procs[i].state == PROC_VFS_DEFERRED && procs[i].deferred_reply_slot != 0 {
            let reply_slot = procs[i].deferred_reply_slot;
            let mrs = procs[i].deferred_mrs;
            procs[i].deferred_reply_slot = 0;
            procs[i].deferred_mrs = [0; 64];
            procs[i].state = PROC_RUNNABLE;
            linux::use_deferred_reply_slot(reply_slot);
            let result = handle_linux_syscall(alloc, procs, i, &mrs);
            match result {
                SyscallResult::Reply(ret) => {
                    linux::use_deferred_reply_slot(0);
                    let mut reply_mrs = host_arch::syscall_reply_frame(&mrs);
                    host_arch::set_syscall_return_value(&mut reply_mrs, ret as u64);
                    reply_caps::send_and_release(
                        reply_slot,
                        msg_info(0, 0, 0, host_arch::FAULT_REPLY_WORDS as u64),
                        &reply_mrs,
                    );
                }
                SyscallResult::ReplyFrame(frame) => {
                    linux::use_deferred_reply_slot(0);
                    reply_caps::send_and_release(
                        reply_slot,
                        msg_info(0, 0, 0, host_arch::FAULT_REPLY_WORDS as u64),
                        &frame,
                    );
                }
                SyscallResult::Block => {}
                SyscallResult::Stop => {
                    linux::use_deferred_reply_slot(0);
                    reply_caps::stop_and_release(reply_slot);
                }
            }
            return;
        }
        i += 1;
    }
}

fn find_proc_by_pid(procs: &[TaskStruct; MAX_PROCS], pid: u64) -> Option<usize> {
    for i in 0..MAX_PROCS {
        if procs[i].pid == pid && procs[i].state != PROC_UNUSED {
            return Some(i);
        }
    }
    None
}

type FrameMap = (u64, u64, bool, bool, u64);

fn spawn_service_servers(alloc: &mut Allocator, fault_ep: u64) -> u64 {
    let uart_ep = alloc.retype_one(OBJ_ENDPOINT, 0);
    let vfs_ep = alloc.retype_one(OBJ_ENDPOINT, 0);
    let uart_mmio_frame = if UART0_MMIO_FRAME_BASE != 0 {
        alloc.retype_device_4k_at(UART0_MMIO_FRAME_BASE)
    } else {
        0
    };
    let mut shared_frames = [0u64; SHARED_BUFFER_PAGES];
    let mut page = 0usize;
    while page < SHARED_BUFFER_PAGES {
        shared_frames[page] = alloc.retype_one(OBJ_4K, 0);
        map_shared_frame_into_host(alloc, shared_frames[page], page);
        page += 1;
    }
    let shared_maps = shared_frame_maps(&shared_frames);
    let uart_maps = [(uart_mmio_frame, UART_MMIO_FRAME_VADDR, true, false, 0)];
    let uart_maps: &[FrameMap] = if uart_mmio_frame == 0 {
        &[]
    } else {
        &uart_maps
    };
    spawn_service_server(
        alloc,
        UART_SERVER_PID,
        UART_SERVER_ELF,
        uart_ep,
        IpcBadge::UartServer.raw(),
        "uart-server",
        fault_ep,
        &[(
            UART_REPLY_ENDPOINT_CPTR,
            vfs_ep,
            cap_rights(false, false, false, true),
            IpcBadge::UartReply.raw(),
        )],
        uart_maps,
        SERVICE_UNTYPED_BITS,
        0,
    );
    spawn_service_server(
        alloc,
        VFS_SERVER_PID,
        VFS_SERVER_ELF,
        vfs_ep,
        IpcBadge::VfsServer.raw(),
        "vfs-server",
        fault_ep,
        &[
            (
                UART_ENDPOINT_CPTR,
                uart_ep,
                cap_rights(true, true, true, true),
                IpcBadge::VfsServer.raw(),
            ),
            (
                HOST_REPLY_ENDPOINT_CPTR,
                fault_ep,
                cap_rights(false, false, false, true),
                IpcBadge::VfsReply.raw(),
            ),
        ],
        &shared_maps,
        VFS_SERVICE_UNTYPED_BITS,
        0,
    );
    vfs_ep
}

fn shared_frame_maps(frames: &[u64; SHARED_BUFFER_PAGES]) -> [FrameMap; SHARED_BUFFER_PAGES] {
    [
        (
            frames[0],
            SHARED_BUFFER_VADDR,
            true,
            false,
            VM_ATTR_UNCACHED,
        ),
        (
            frames[1],
            SHARED_BUFFER_VADDR + PAGE_SIZE,
            true,
            false,
            VM_ATTR_UNCACHED,
        ),
        (
            frames[2],
            SHARED_BUFFER_VADDR + PAGE_SIZE * 2,
            true,
            false,
            VM_ATTR_UNCACHED,
        ),
        (
            frames[3],
            SHARED_BUFFER_VADDR + PAGE_SIZE * 3,
            true,
            false,
            VM_ATTR_UNCACHED,
        ),
    ]
}

fn map_shared_frame_into_host(alloc: &mut Allocator, frame_slot: u64, page: usize) {
    let vaddr = SHARED_BUFFER_VADDR + page as u64 * PAGE_SIZE;
    ensure_host_page_table_path(alloc, vaddr);
    call_checked(
        frame_slot,
        LABEL_PAGE_MAP,
        &[INIT_VSPACE],
        &[
            vaddr,
            cap_rights(false, false, true, true),
            1 | VM_ATTR_UNCACHED,
        ],
    );
}

fn spawn_service_server(
    alloc: &mut Allocator,
    pid: u64,
    elf: &[u8],
    service_ep: u64,
    endpoint_badge: u64,
    name: &str,
    fault_ep: u64,
    extra_caps: &[(u64, u64, u64, u64)],
    mapped_frames: &[FrameMap],
    untyped_bits: u64,
    bound_notification: u64,
) {
    let service_untyped = alloc.retype_one(OBJ_UNTYPED, untyped_bits);
    let mut service = create_child_from_untyped(alloc, pid, 0, fault_ep, service_untyped);
    mint_cap_to_child(
        &service,
        SERVICE_ENDPOINT_CPTR,
        service_ep,
        cap_rights(true, true, true, true),
        endpoint_badge,
    );
    mint_cap_to_child(
        &service,
        SERVER_CNODE_CPTR,
        service.cnode,
        cap_rights(true, true, true, true),
        cnode_cap_data(0, consts::WORD_BITS - consts::CHILD_CNODE_BITS),
    );
    let recv_reply = alloc.retype_one_from(service_untyped, OBJ_REPLY, 0);
    mint_cap_to_child(
        &service,
        SERVER_RECV_REPLY_CPTR,
        recv_reply,
        cap_rights(true, true, true, true),
        0,
    );
    for &(dst_cptr, src_cap, rights, badge) in extra_caps {
        mint_cap_to_child(&service, dst_cptr, src_cap, rights, badge);
    }
    load_elf(alloc, &mut service, elf);
    map_stack_pages(alloc, &mut service, consts::SERVICE_STACK_PAGES);
    for &(frame_slot, va, writable, executable, extra_attrs) in mapped_frames {
        map_existing_child_frame_with_attrs(
            alloc,
            &service,
            frame_slot,
            va,
            writable,
            executable,
            extra_attrs,
        );
    }
    if bound_notification != 0 {
        call_checked(
            service.tcb,
            LABEL_TCB_BIND_NOTIFICATION,
            &[bound_notification],
            &[],
        );
    }
    start_child_with_a0_a1(&service, consts::CHILD_IPC_BUFFER, 0);
    info!("linux-compat: spawned {} pid={}", name, pid);
}

fn init_vfs_server(vfs_ep: u64) {
    info!("linux-compat: init vfs server");
    let reply = unsafe {
        sel4_call(
            vfs_ep,
            msg_info(VfsOp::Init.raw(), 0, 0, 2),
            &[IpcProtocol::HostToVfs.raw(), LINUX_ABI_VERSION],
        )
    };
    let label = msg_label(reply.info);
    if label != 0 || reply.mrs[0] != IpcStatus::Ok.raw() {
        warn!(
            "linux-compat: vfs init failed label={} status={}",
            label, reply.mrs[0]
        );
        halt_loop();
    }
    info!("linux-compat: vfs ramfs ready");
}

fn setup_timer_notification(alloc: &mut Allocator) {
    let ntfn = alloc.retype_one(OBJ_NOTIFICATION, 0);
    let irq_handler = alloc.alloc_slot();
    call_checked(
        IRQ_CONTROL,
        LABEL_IRQ_ISSUE_IRQ_HANDLER,
        &[ROOT_CNODE],
        &[KERNEL_TIMER_IRQ, irq_handler, ROOT_CNODE_DEPTH],
    );
    call_checked(irq_handler, LABEL_IRQ_SET_NOTIFICATION, &[ntfn], &[]);
    call_checked(INIT_TCB, LABEL_TCB_BIND_NOTIFICATION, &[ntfn], &[]);
}

#[panic_handler]
fn panic(_info: &PanicInfo) -> ! {
    error!("linux-compat: panic");
    halt_loop()
}
