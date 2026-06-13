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
mod disk_transport;
mod exec_syscalls;
mod fs_syscalls;
mod io_syscalls;
mod memory_syscalls;
mod process_syscalls;
mod types;
mod util;
mod vfs;
mod xv6;

use crate::arch::current as host_arch;
use allocator::Allocator;
use child::{
    create_child, create_child_from_untyped, frame_paddr, load_elf, load_payload,
    map_existing_child_frame, map_stack, map_stack_pages, mint_cap_to_child, start_child,
    start_child_with_a0_a1,
};
use consts::{
    DISK_SERVER_ELF, DISK_SERVER_PID, FAULT_UNKNOWN_SYSCALL, FAULT_VM_FAULT,
    LABEL_IRQ_ISSUE_IRQ_HANDLER, LABEL_IRQ_SET_NOTIFICATION, LABEL_PAGE_MAP, UART_SERVER_ELF,
    UART_SERVER_PID, VFS_SERVER_ELF, VFS_SERVER_PID, VIRTIO_BLK_SECTOR_SIZE, VfsOp,
    XV6_ABI_VERSION, XV6_DISK_COMPLETION_NTFN_CPTR, XV6_DISK_COMPLETION_RING_VADDR,
    XV6_DISK_ENDPOINT_CPTR, XV6_DISK_SHARED_BUFFER_PAGES, XV6_DISK_SHARED_BUFFER_VADDR,
    XV6_HOST_REPLY_ENDPOINT_CPTR, XV6_UART_ENDPOINT_CPTR, XV6_UART_MMIO_FRAME_VADDR,
    XV6_UART_REPLY_ENDPOINT_CPTR, XV6_VIRTIO_DMA_VADDR, XV6_XV6FS_ENDPOINT_CPTR, XV6FS_SERVER_ELF,
    XV6FS_SERVER_PID, Xv6Badge, Xv6Protocol, Xv6Status,
};
use consts::{
    INIT_TCB, INIT_VSPACE, IRQ_CONTROL, KERNEL_TIMER_IRQ, MAX_PROCS, OBJ_4K, OBJ_ENDPOINT,
    OBJ_NOTIFICATION, OBJ_REPLY, OBJ_UNTYPED,
};
use consts::{LABEL_CNODE_SAVE_CALLER, LABEL_TCB_BIND_NOTIFICATION, PAGE_SIZE, ROOT_CNODE_DEPTH};
use consts::{
    PROC_RUNNABLE, PROC_UNUSED, PROC_VFS_DEFERRED, ROOT_CNODE, SERVICE_UNTYPED_BITS,
    XV6_SERVER_RECV_REPLY_CPTR, XV6_SERVICE_ENDPOINT_CPTR,
};
use sel4_user::{
    call_checked, cap_rights, cnode_cap_data, init_ipc_buffer, msg_info, msg_label, sel4_call,
    sel4_recv, sel4_reply_recv,
};
use types::{BootInfo, SyscallResult, TaskStruct};
use util::{error, halt_loop, info, init_logger, warn};
use xv6::{handle_xv6_fault, handle_xv6_syscall};

static SAW_FAULT_IPC: AtomicBool = AtomicBool::new(false);

struct ProcessTable {
    procs: UnsafeCell<[TaskStruct; MAX_PROCS]>,
}

// xv6-host mutates the process table from the single rootserver fault loop.
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
    info!("xv6-host: boot");

    let mut alloc = Allocator::new(bi);
    let fault_ep = alloc.retype_one(OBJ_ENDPOINT, 0);
    let procs = PROCESS_TABLE.procs();
    let service_eps = spawn_service_servers(&mut alloc, fault_ep);
    init_service_servers(service_eps.vfs);
    xv6::init_vfs_client(service_eps.vfs);
    procs[0] = create_child(&mut alloc, 0, 1, 0, fault_ep);
    xv6::init_vfs_process(&mut procs[0]);
    setup_timer_notification(&mut alloc);
    load_payload(&mut alloc, &mut procs[0]);
    map_stack(&mut alloc, &mut procs[0]);
    start_child(&procs[0]);

    info!("xv6-host: waiting for fault IPC");
    let mut reply_pending = false;
    let mut reply_info = msg_info(0, 0, 0, host_arch::FAULT_REPLY_WORDS as u64);
    let mut reply_mrs = [0u64; host_arch::FAULT_REPLY_WORDS];
    loop {
        let msg = if reply_pending {
            reply_pending = false;
            unsafe { sel4_reply_recv(fault_ep, reply_info, &reply_mrs) }
        } else {
            unsafe { sel4_recv(fault_ep) }
        };

        if (msg.badge & Xv6Badge::VfsReply.raw()) != 0 {
            if let Some(pump_waiters) = xv6::complete_vfs_async_reply(&mut alloc, procs, &msg) {
                if pump_waiters {
                    xv6::pump_vfs_waiters(&mut alloc, procs);
                }
                xv6::pump_sleep_waiters(&mut alloc, procs);
                pump_deferred_vfs_syscalls(&mut alloc, procs);
                continue;
            }
        }

        let label = msg_label(msg.info);
        if label == 0 {
            xv6::tick();
            xv6::pump_vfs_waiters(&mut alloc, procs);
            xv6::pump_sleep_waiters(&mut alloc, procs);
            continue;
        }
        let Some(proc_idx) = find_proc_by_pid(&procs, msg.badge) else {
            warn!("xv6-host: fault from unknown pid={}", msg.badge);
            halt_loop();
        };

        if label == FAULT_UNKNOWN_SYSCALL
            && xv6::has_active_vfs_async_requests()
            && xv6::should_defer_vfs_syscall(&procs[proc_idx], &msg.mrs)
        {
            defer_vfs_syscall(&mut alloc, &mut procs[proc_idx], &msg.mrs);
            continue;
        }

        let result = if label == FAULT_UNKNOWN_SYSCALL {
            if !SAW_FAULT_IPC.swap(true, Ordering::Relaxed) {
                info!("xv6-host: UnknownSyscall fault IPC");
            }
            handle_xv6_syscall(&mut alloc, procs, proc_idx, &msg.mrs)
        } else {
            if label != FAULT_VM_FAULT {
                warn!("xv6-host: non-syscall fault label={}", label);
            }
            handle_xv6_fault(&mut alloc, procs, proc_idx, label, &msg.mrs)
        };
        xv6::pump_vfs_waiters(&mut alloc, procs);
        xv6::pump_sleep_waiters(&mut alloc, procs);

        match result {
            SyscallResult::Reply(ret) => {
                reply_info = msg_info(0, 0, 0, host_arch::FAULT_REPLY_WORDS as u64);
                reply_mrs = host_arch::syscall_reply_frame(&msg.mrs);
                host_arch::set_syscall_return_value(&mut reply_mrs, ret as u64);
                reply_pending = true;
            }
            SyscallResult::ReplyFrame(frame) => {
                reply_info = msg_info(0, 0, 0, host_arch::FAULT_REPLY_WORDS as u64);
                reply_mrs = frame;
                reply_pending = true;
            }
            SyscallResult::Block => {
                reply_pending = false;
            }
            SyscallResult::Stop => {
                reply_info = msg_info(1, 0, 0, 0);
                reply_mrs = [0; host_arch::FAULT_REPLY_WORDS];
                reply_pending = true;
            }
        }
        pump_deferred_vfs_syscalls(&mut alloc, procs);
    }
}

fn defer_vfs_syscall(alloc: &mut Allocator, child: &mut TaskStruct, mrs: &[u64; 64]) {
    let reply_slot = alloc.alloc_slot();
    call_checked(
        ROOT_CNODE,
        LABEL_CNODE_SAVE_CALLER,
        &[],
        &[reply_slot, ROOT_CNODE_DEPTH],
    );
    child.deferred_reply_slot = reply_slot;
    child.deferred_mrs = *mrs;
    child.state = PROC_VFS_DEFERRED;
}

fn pump_deferred_vfs_syscalls(alloc: &mut Allocator, procs: &mut [TaskStruct; MAX_PROCS]) {
    if xv6::has_active_vfs_async_requests() {
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
            xv6::use_deferred_reply_slot(reply_slot);
            let result = handle_xv6_syscall(alloc, procs, i, &mrs);
            match result {
                SyscallResult::Reply(ret) => {
                    xv6::use_deferred_reply_slot(0);
                    let mut reply_mrs = host_arch::syscall_reply_frame(&mrs);
                    host_arch::set_syscall_return_value(&mut reply_mrs, ret as u64);
                    unsafe {
                        sel4_user::sel4_send(
                            reply_slot,
                            msg_info(0, 0, 0, host_arch::FAULT_REPLY_WORDS as u64),
                            &reply_mrs,
                        );
                    }
                    alloc.delete_cap_slot(reply_slot);
                }
                SyscallResult::ReplyFrame(frame) => {
                    xv6::use_deferred_reply_slot(0);
                    unsafe {
                        sel4_user::sel4_send(
                            reply_slot,
                            msg_info(0, 0, 0, host_arch::FAULT_REPLY_WORDS as u64),
                            &frame,
                        );
                    }
                    alloc.delete_cap_slot(reply_slot);
                }
                SyscallResult::Block => {}
                SyscallResult::Stop => {
                    xv6::use_deferred_reply_slot(0);
                    unsafe {
                        sel4_user::sel4_send(reply_slot, msg_info(1, 0, 0, 0), &[]);
                    }
                    alloc.delete_cap_slot(reply_slot);
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

struct ServiceEndpoints {
    vfs: u64,
}

const MAX_DISK_MAPS: usize = 1 + 1 + XV6_DISK_SHARED_BUFFER_PAGES + 1;

fn spawn_service_servers(alloc: &mut Allocator, fault_ep: u64) -> ServiceEndpoints {
    let uart_ep = alloc.retype_one(OBJ_ENDPOINT, 0);
    let disk_ep = alloc.retype_one(OBJ_ENDPOINT, 0);
    let xv6fs_ep = alloc.retype_one(OBJ_ENDPOINT, 0);
    let vfs_ep = alloc.retype_one(OBJ_ENDPOINT, 0);
    let disk_irq_ntfn = alloc.retype_one(OBJ_NOTIFICATION, 0);
    let disk_irq_handler = disk_transport::issue_irq_handler(alloc, disk_irq_ntfn);
    let uart_mmio_frame = alloc.retype_device_4k_at(consts::UART0_MMIO_FRAME_BASE);
    let virtio_dma_frame = alloc.retype_one(OBJ_4K, 0);
    let virtio_dma_paddr = frame_paddr(virtio_dma_frame);
    let mut disk_shared_frames = [0u64; XV6_DISK_SHARED_BUFFER_PAGES];
    let mut page = 0usize;
    while page < XV6_DISK_SHARED_BUFFER_PAGES {
        disk_shared_frames[page] = alloc.retype_one(OBJ_4K, 0);
        map_shared_frame_into_host(disk_shared_frames[page], page);
        page += 1;
    }
    let disk_completion_ntfn = alloc.retype_one(OBJ_NOTIFICATION, 0);
    let disk_completion_ring_frame = alloc.retype_one(OBJ_4K, 0);
    let shared_maps = shared_frame_maps(&disk_shared_frames);
    let mut disk_maps: [disk_transport::FrameMap; MAX_DISK_MAPS] =
        [(0, 0, false, false); MAX_DISK_MAPS];
    let mut disk_maps_len = 0usize;
    if let Some(device_map) = disk_transport::device_frame_map(alloc) {
        push_disk_map(&mut disk_maps, &mut disk_maps_len, device_map);
    }
    push_disk_map(
        &mut disk_maps,
        &mut disk_maps_len,
        (virtio_dma_frame, XV6_VIRTIO_DMA_VADDR, true, false),
    );
    push_disk_map(&mut disk_maps, &mut disk_maps_len, shared_maps[0]);
    push_disk_map(&mut disk_maps, &mut disk_maps_len, shared_maps[1]);
    push_disk_map(&mut disk_maps, &mut disk_maps_len, shared_maps[2]);
    push_disk_map(&mut disk_maps, &mut disk_maps_len, shared_maps[3]);
    push_disk_map(
        &mut disk_maps,
        &mut disk_maps_len,
        (
            disk_completion_ring_frame,
            XV6_DISK_COMPLETION_RING_VADDR,
            true,
            false,
        ),
    );
    let xv6fs_maps = [
        shared_maps[0],
        shared_maps[1],
        shared_maps[2],
        shared_maps[3],
        (
            disk_completion_ring_frame,
            XV6_DISK_COMPLETION_RING_VADDR,
            true,
            false,
        ),
    ];
    let vfs_maps = [
        shared_maps[0],
        shared_maps[1],
        shared_maps[2],
        shared_maps[3],
    ];
    spawn_service_server(
        alloc,
        UART_SERVER_PID,
        UART_SERVER_ELF,
        uart_ep,
        Xv6Badge::UartServer.raw(),
        "uart-server",
        fault_ep,
        &[(
            XV6_UART_REPLY_ENDPOINT_CPTR,
            vfs_ep,
            cap_rights(false, false, false, true),
            Xv6Badge::UartReply.raw(),
        )],
        &[(uart_mmio_frame, XV6_UART_MMIO_FRAME_VADDR, true, false)],
        0,
        0,
    );
    spawn_service_server(
        alloc,
        DISK_SERVER_PID,
        DISK_SERVER_ELF,
        disk_ep,
        Xv6Badge::DiskServer.raw(),
        "virtio-disk-server",
        fault_ep,
        &[
            (
                consts::XV6_DISK_IRQ_NTFN_CPTR,
                disk_irq_ntfn,
                cap_rights(false, false, true, false),
                Xv6Badge::DiskIrq.raw(),
            ),
            (
                consts::XV6_DISK_IRQ_HANDLER_CPTR,
                disk_irq_handler,
                cap_rights(true, true, true, true),
                0,
            ),
            (
                XV6_DISK_COMPLETION_NTFN_CPTR,
                disk_completion_ntfn,
                cap_rights(false, false, false, true),
                Xv6Badge::DiskCompletion.raw(),
            ),
        ],
        &disk_maps[..disk_maps_len],
        virtio_dma_paddr,
        disk_irq_ntfn,
    );
    spawn_service_server(
        alloc,
        XV6FS_SERVER_PID,
        XV6FS_SERVER_ELF,
        xv6fs_ep,
        Xv6Badge::Xv6FsServer.raw(),
        "xv6fs-server",
        fault_ep,
        &[
            (
                XV6_DISK_ENDPOINT_CPTR,
                disk_ep,
                cap_rights(true, true, true, true),
                Xv6Badge::Xv6FsServer.raw(),
            ),
            (
                XV6_DISK_COMPLETION_NTFN_CPTR,
                disk_completion_ntfn,
                cap_rights(false, false, true, false),
                0,
            ),
            (
                consts::XV6_VFS_REPLY_ENDPOINT_CPTR,
                vfs_ep,
                cap_rights(false, false, false, true),
                Xv6Badge::Xv6FsReply.raw(),
            ),
        ],
        &xv6fs_maps,
        0,
        disk_completion_ntfn,
    );
    spawn_service_server(
        alloc,
        VFS_SERVER_PID,
        VFS_SERVER_ELF,
        vfs_ep,
        Xv6Badge::VfsServer.raw(),
        "vfs-server",
        fault_ep,
        &[
            (
                XV6_XV6FS_ENDPOINT_CPTR,
                xv6fs_ep,
                cap_rights(true, true, true, true),
                Xv6Badge::VfsServer.raw(),
            ),
            (
                XV6_UART_ENDPOINT_CPTR,
                uart_ep,
                cap_rights(true, true, true, true),
                Xv6Badge::VfsServer.raw(),
            ),
            (
                XV6_HOST_REPLY_ENDPOINT_CPTR,
                fault_ep,
                cap_rights(false, false, false, true),
                Xv6Badge::VfsReply.raw(),
            ),
        ],
        &vfs_maps,
        0,
        0,
    );
    ServiceEndpoints { vfs: vfs_ep }
}

fn shared_frame_maps(
    frames: &[u64; XV6_DISK_SHARED_BUFFER_PAGES],
) -> [(u64, u64, bool, bool); XV6_DISK_SHARED_BUFFER_PAGES] {
    [
        (frames[0], XV6_DISK_SHARED_BUFFER_VADDR, true, false),
        (
            frames[1],
            XV6_DISK_SHARED_BUFFER_VADDR + PAGE_SIZE,
            true,
            false,
        ),
        (
            frames[2],
            XV6_DISK_SHARED_BUFFER_VADDR + PAGE_SIZE * 2,
            true,
            false,
        ),
        (
            frames[3],
            XV6_DISK_SHARED_BUFFER_VADDR + PAGE_SIZE * 3,
            true,
            false,
        ),
    ]
}

fn push_disk_map(
    maps: &mut [disk_transport::FrameMap; MAX_DISK_MAPS],
    len: &mut usize,
    map: disk_transport::FrameMap,
) {
    if *len >= MAX_DISK_MAPS {
        warn!("xv6-host: disk frame map table exhausted");
        halt_loop();
    }
    maps[*len] = map;
    *len += 1;
}

fn map_shared_frame_into_host(frame_slot: u64, page: usize) {
    call_checked(
        frame_slot,
        LABEL_PAGE_MAP,
        &[INIT_VSPACE],
        &[
            XV6_DISK_SHARED_BUFFER_VADDR + page as u64 * PAGE_SIZE,
            cap_rights(false, false, true, true),
            1,
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
    mapped_frames: &[(u64, u64, bool, bool)],
    start_a1: u64,
    bound_notification: u64,
) {
    let service_untyped = alloc.retype_one(OBJ_UNTYPED, SERVICE_UNTYPED_BITS);
    let mut service = create_child_from_untyped(alloc, pid, 0, fault_ep, service_untyped);
    mint_cap_to_child(
        &service,
        XV6_SERVICE_ENDPOINT_CPTR,
        service_ep,
        cap_rights(true, true, true, true),
        endpoint_badge,
    );
    mint_cap_to_child(
        &service,
        consts::XV6_SERVER_CNODE_CPTR,
        service.cnode,
        cap_rights(true, true, true, true),
        cnode_cap_data(0, consts::WORD_BITS - consts::CHILD_CNODE_BITS),
    );
    let recv_reply = alloc.retype_one_from(service_untyped, OBJ_REPLY, 0);
    mint_cap_to_child(
        &service,
        XV6_SERVER_RECV_REPLY_CPTR,
        recv_reply,
        cap_rights(true, true, true, true),
        0,
    );
    for &(dst_cptr, src_cap, rights, badge) in extra_caps {
        mint_cap_to_child(&service, dst_cptr, src_cap, rights, badge);
    }
    load_elf(alloc, &mut service, elf);
    map_stack_pages(alloc, &mut service, consts::SERVICE_STACK_PAGES);
    for &(frame_slot, va, writable, executable) in mapped_frames {
        map_existing_child_frame(alloc, &service, frame_slot, va, writable, executable);
    }
    if bound_notification != 0 {
        call_checked(
            service.tcb,
            LABEL_TCB_BIND_NOTIFICATION,
            &[bound_notification],
            &[],
        );
    }
    start_child_with_a0_a1(&service, consts::CHILD_IPC_BUFFER, start_a1);
    info!("xv6-host: spawned {} pid={}", name, pid);
}

fn init_service_servers(vfs_ep: u64) {
    info!("xv6-host: init vfs server");
    let reply = unsafe {
        sel4_call(
            vfs_ep,
            msg_info(VfsOp::Init.raw(), 0, 0, 2),
            &[Xv6Protocol::HostToVfs.raw(), XV6_ABI_VERSION],
        )
    };
    let label = msg_label(reply.info);
    if label != 0 || reply.mrs[0] != Xv6Status::Ok.raw() {
        warn!(
            "xv6-host: vfs init failed label={} status={}",
            label, reply.mrs[0]
        );
        halt_loop();
    }
    if reply.mrs[1] != VIRTIO_BLK_SECTOR_SIZE as u64 || reply.mrs[2] == 0 {
        warn!("xv6-host: vfs init returned invalid geometry");
        halt_loop();
    }
    info!(
        "xv6-host: vfs server ready sector={} block={} disk-blocks={}",
        reply.mrs[1], reply.mrs[2], reply.mrs[3]
    );
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
    error!("xv6-host: panic");
    halt_loop()
}
