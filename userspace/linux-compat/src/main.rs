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
mod host;
mod io_syscalls;
mod linux;
mod memory_syscalls;
mod process_syscalls;
mod reply_caps;
mod threads;
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
    OBJ_NOTIFICATION, OBJ_UNTYPED,
};
use consts::{LABEL_TCB_BIND_NOTIFICATION, PAGE_SIZE, ROOT_CNODE_DEPTH};
use consts::{
    PROC_RUNNABLE, PROC_WAITING, ROOT_CNODE, SERVER_CNODE_CPTR, SERVICE_UNTYPED_BITS,
    VM_ATTR_UNCACHED,
};
use exec_syscalls::load_init_program;
use linux::handle_linux_syscall;
use sel4_user::{
    call_checked, cap_rights, cnode_cap_data, init_ipc_buffer, msg_info, msg_label, rt, sel4_call,
    sel4_recv,
};
use types::{BootInfo, SyscallResult, TaskStruct};
use util::{error, halt_loop, info, init_logger, warn};

static SAW_FAULT_IPC: AtomicBool = AtomicBool::new(false);

struct ProcessTable {
    procs: UnsafeCell<[TaskStruct; MAX_PROCS]>,
}

// Process table mutation is serialized by `host::with_host`.
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

pub(crate) static PROCESS_TABLE: ProcessTable = ProcessTable::new();

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
    let wake_ntfn = alloc.retype_one(OBJ_NOTIFICATION, 0);
    threads::install_main_tls(bi.ipc_buffer);
    threads::start_workers(&mut alloc, wake_ntfn, bi.init_thread_cnode_size_bits);
    host::init(alloc);
    rt::set_wake_notification(wake_ntfn);
    rt::run(|| reactor_idle(fault_ep));
}

fn reactor_idle(fault_ep: u64) {
    let reply_slot = reply_caps::acquire();
    let msg = unsafe { sel4_recv(fault_ep) };
    reply_caps::save_caller(reply_slot);

    if (msg.badge & IpcBadge::VfsReply.raw()) != 0 {
        let _ = linux::complete_vfs_async_reply(&msg);
        reply_caps::release(reply_slot);
        return;
    }

    let label = msg_label(msg.info);
    if label == 0 {
        reply_caps::release(reply_slot);
        linux::tick();
        rt::time::advance(linux::ticks_now());
        return;
    }

    let pid = msg.badge;
    let mrs = msg.mrs;
    rt::spawn(async move {
        process_fault(pid, label, mrs, reply_slot).await;
    });
}

async fn process_fault(pid: u64, label: u64, mrs: [u64; 64], reply_slot: u64) {
    if host::with_host(|_, procs| host::find_proc(procs, pid)).is_none() {
        warn!("linux-compat: fault from unknown pid={}", pid);
        halt_loop();
    }

    let result = if label == FAULT_UNKNOWN_SYSCALL {
        if !SAW_FAULT_IPC.swap(true, Ordering::Relaxed) {
            info!("linux-compat: UnknownSyscall fault IPC");
        }
        handle_linux_syscall(pid, &mrs, reply_slot).await
    } else {
        if label != FAULT_VM_FAULT {
            warn!("linux-compat: non-syscall fault label={}", label);
        }
        linux::handle_linux_fault(pid, label, &mrs).await
    };

    finish_syscall(pid, &mrs, reply_slot, result);
}

fn finish_syscall(pid: u64, mrs: &[u64; 64], reply_slot: u64, result: SyscallResult) {
    match result {
        SyscallResult::Reply(ret) => {
            if !process_can_reply(pid) {
                reply_caps::stop_and_release(reply_slot);
                return;
            }
            let mut reply_mrs = host_arch::syscall_reply_frame(mrs);
            host_arch::set_syscall_return_value(&mut reply_mrs, ret as u64);
            reply_caps::send_and_release(
                reply_slot,
                msg_info(0, 0, 0, host_arch::FAULT_REPLY_WORDS as u64),
                &reply_mrs,
            );
        }
        SyscallResult::ReplyFrame(frame) => {
            if !process_can_reply(pid) {
                reply_caps::stop_and_release(reply_slot);
                return;
            }
            reply_caps::send_and_release(
                reply_slot,
                msg_info(0, 0, 0, host_arch::FAULT_REPLY_WORDS as u64),
                &frame,
            );
        }
        SyscallResult::Block => {}
        SyscallResult::Stop => {
            reply_caps::stop_and_release(reply_slot);
        }
    }
}

fn process_can_reply(pid: u64) -> bool {
    host::with_host(|_, procs| {
        host::find_proc(procs, pid)
            .map(|idx| procs[idx].state == PROC_RUNNABLE || procs[idx].state == PROC_WAITING)
            .unwrap_or(false)
    })
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
