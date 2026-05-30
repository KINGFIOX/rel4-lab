#![no_std]
#![no_main]
#![deny(unsafe_attr_outside_unsafe)]
#![deny(unsafe_op_in_unsafe_fn)]

use core::panic::PanicInfo;
use core::ptr;

mod allocator;
mod child;
mod consts;
mod sel4;
mod types;
mod util;
mod xv6;

use allocator::Allocator;
use child::{
    create_child, create_child_from_untyped, frame_paddr, load_elf, load_payload,
    map_existing_child_frame, map_stack, mint_cap_to_child, start_child, start_child_with_a0_a1,
};
use consts::{
    DISK_SERVER_ELF, DISK_SERVER_PID, DISK_SERVER_PROC_SLOT, FAULT_UNKNOWN_SYSCALL, FAULT_VM_FAULT,
    FS_OP_INIT, FS_SERVER_ELF, FS_SERVER_PID, FS_SERVER_PROC_SLOT, LABEL_IRQ_ISSUE_IRQ_HANDLER,
    LABEL_IRQ_SET_NOTIFICATION, VIRTIO_BLK_SECTOR_SIZE, XV6_ABI_VERSION, XV6_DISK_ENDPOINT_CPTR,
    XV6_DISK_SHARED_BUFFER_VADDR, XV6_HOST_TO_FS_PROTOCOL, XV6_OK, XV6_VIRTIO_DMA_VADDR,
    XV6_VIRTIO_MMIO_VADDR,
};
use consts::{
    INIT_TCB, IRQ_CONTROL, KERNEL_TIMER_IRQ, MAX_PROCS, OBJ_4K, OBJ_ENDPOINT, OBJ_NOTIFICATION,
    OBJ_UNTYPED,
};
use consts::{LABEL_TCB_BIND_NOTIFICATION, ROOT_CNODE_DEPTH};
use consts::{
    PROC_UNUSED, ROOT_CNODE, SERVICE_UNTYPED_BITS, XV6_DISK_SERVER_BADGE, XV6_FS_SERVER_BADGE,
    XV6_SERVICE_ENDPOINT_CPTR,
};
use sel4::{
    call_checked, cap_rights, init_ipc_buffer, msg_info, msg_label, sel4_call, sel4_recv,
    sel4_reply_recv,
};
use types::{BootInfo, Child};
use util::{halt_loop, log, print_u64};
use xv6::{SyscallResult, handle_xv6_fault, handle_xv6_syscall};

static mut SAW_FAULT_IPC: bool = false;

#[unsafe(no_mangle)]
pub extern "C" fn _start(bootinfo: usize) -> ! {
    unsafe {
        clear_bss();
    }
    run(bootinfo as *const BootInfo);
}

unsafe fn clear_bss() {
    unsafe extern "C" {
        static mut __bss_start: u8;
        static mut __bss_end: u8;
    }
    unsafe {
        let start = core::ptr::addr_of_mut!(__bss_start) as usize;
        let end = core::ptr::addr_of_mut!(__bss_end) as usize;
        ptr::write_bytes(start as *mut u8, 0, end.saturating_sub(start));
    }
}

fn run(bi_ptr: *const BootInfo) -> ! {
    let bi = unsafe { &*bi_ptr };
    init_ipc_buffer(bi.ipc_buffer);
    log("xv6-host: boot\n");

    let mut alloc = Allocator::new(bi);
    let fault_ep = alloc.retype_one(OBJ_ENDPOINT, 0);
    let mut procs = [Child::empty(); MAX_PROCS];
    let service_eps = spawn_service_servers(&mut alloc, fault_ep);
    init_service_servers(service_eps.fs);
    procs[0] = create_child(&mut alloc, 0, 1, 0, fault_ep);
    xv6::init_fds(&mut procs[0]);
    setup_timer_notification(&mut alloc);
    load_payload(&mut alloc, &mut procs[0]);
    map_stack(&mut alloc, &mut procs[0]);
    start_child(&procs[0]);

    log("xv6-host: waiting for fault IPC\n");
    let mut reply_pending = false;
    let mut reply_info = msg_info(0, 0, 0, 11);
    let mut reply_mrs = [0u64; 11];
    loop {
        let msg = if reply_pending {
            reply_pending = false;
            unsafe { sel4_reply_recv(fault_ep, reply_info, &reply_mrs) }
        } else {
            unsafe { sel4_recv(fault_ep) }
        };

        let label = msg_label(msg.info);
        if label == 0 {
            xv6::tick();
            continue;
        }
        let Some(proc_idx) = find_proc_by_pid(&procs, msg.badge) else {
            log("xv6-host: fault from unknown pid=");
            print_u64(msg.badge);
            log("\n");
            halt_loop();
        };

        let result = if label == FAULT_UNKNOWN_SYSCALL {
            unsafe {
                if !SAW_FAULT_IPC {
                    SAW_FAULT_IPC = true;
                    log("xv6-host: UnknownSyscall fault IPC\n");
                }
            }
            handle_xv6_syscall(&mut alloc, &mut procs, proc_idx, &msg.mrs)
        } else {
            if label != FAULT_VM_FAULT {
                log("xv6-host: non-syscall fault label=");
                print_u64(label);
                log("\n");
            }
            handle_xv6_fault(&mut alloc, &mut procs, proc_idx, label, &msg.mrs)
        };

        match result {
            SyscallResult::Reply(ret) => {
                reply_info = msg_info(0, 0, 0, 11);
                reply_mrs = msg.mrs[..11].try_into().unwrap_or([0; 11]);
                reply_mrs[0] = msg.mrs[0].wrapping_add(4);
                reply_mrs[3] = ret as u64;
                reply_pending = true;
            }
            SyscallResult::ReplyFrame(frame) => {
                reply_info = msg_info(0, 0, 0, 11);
                reply_mrs = frame;
                reply_pending = true;
            }
            SyscallResult::Block => {
                reply_pending = false;
            }
            SyscallResult::Stop => {
                reply_info = msg_info(1, 0, 0, 0);
                reply_mrs = [0; 11];
                reply_pending = true;
            }
        }
    }
}

fn find_proc_by_pid(procs: &[Child; MAX_PROCS], pid: u64) -> Option<usize> {
    for i in 0..MAX_PROCS {
        if procs[i].pid == pid && procs[i].state != PROC_UNUSED {
            return Some(i);
        }
    }
    None
}

struct ServiceEndpoints {
    fs: u64,
}

fn spawn_service_servers(alloc: &mut Allocator, fault_ep: u64) -> ServiceEndpoints {
    let disk_ep = alloc.retype_one(OBJ_ENDPOINT, 0);
    let virtio_mmio_frame = alloc.retype_device_4k_at(consts::VIRTIO_MMIO_BASE);
    let virtio_dma_frame = alloc.retype_one(OBJ_4K, 0);
    let virtio_dma_paddr = frame_paddr(virtio_dma_frame);
    let disk_shared_frame = alloc.retype_one(OBJ_4K, 0);
    spawn_service_server(
        alloc,
        DISK_SERVER_PROC_SLOT,
        DISK_SERVER_PID,
        DISK_SERVER_ELF,
        disk_ep,
        XV6_DISK_SERVER_BADGE,
        "virtio-disk-server",
        fault_ep,
        None,
        &[
            (virtio_mmio_frame, XV6_VIRTIO_MMIO_VADDR, true, false),
            (virtio_dma_frame, XV6_VIRTIO_DMA_VADDR, true, false),
            (disk_shared_frame, XV6_DISK_SHARED_BUFFER_VADDR, true, false),
        ],
        virtio_dma_paddr,
    );
    let fs_ep = alloc.retype_one(OBJ_ENDPOINT, 0);
    spawn_service_server(
        alloc,
        FS_SERVER_PROC_SLOT,
        FS_SERVER_PID,
        FS_SERVER_ELF,
        fs_ep,
        XV6_FS_SERVER_BADGE,
        "xv6-fs-server",
        fault_ep,
        Some((
            XV6_DISK_ENDPOINT_CPTR,
            disk_ep,
            cap_rights(true, true, true, true),
            XV6_FS_SERVER_BADGE,
        )),
        &[(disk_shared_frame, XV6_DISK_SHARED_BUFFER_VADDR, true, false)],
        0,
    );
    ServiceEndpoints { fs: fs_ep }
}

fn spawn_service_server(
    alloc: &mut Allocator,
    proc_slot: usize,
    pid: u64,
    elf: &[u8],
    service_ep: u64,
    endpoint_badge: u64,
    name: &str,
    fault_ep: u64,
    extra_cap: Option<(u64, u64, u64, u64)>,
    mapped_frames: &[(u64, u64, bool, bool)],
    start_a1: u64,
) {
    let service_untyped = alloc.retype_one(OBJ_UNTYPED, SERVICE_UNTYPED_BITS);
    let mut service =
        create_child_from_untyped(alloc, proc_slot, pid, 0, fault_ep, service_untyped);
    mint_cap_to_child(
        &service,
        XV6_SERVICE_ENDPOINT_CPTR,
        service_ep,
        cap_rights(true, true, true, true),
        endpoint_badge,
    );
    if let Some((dst_cptr, src_cap, rights, badge)) = extra_cap {
        mint_cap_to_child(&service, dst_cptr, src_cap, rights, badge);
    }
    load_elf(alloc, &mut service, elf);
    map_stack(alloc, &mut service);
    for &(frame_slot, va, writable, executable) in mapped_frames {
        map_existing_child_frame(alloc, &service, frame_slot, va, writable, executable);
    }
    start_child_with_a0_a1(&service, consts::CHILD_IPC_BUFFER, start_a1);
    log("xv6-host: spawned ");
    log(name);
    log(" pid=");
    print_u64(pid);
    log("\n");
}

fn init_service_servers(fs_ep: u64) {
    log("xv6-host: init fs server\n");
    let reply = unsafe {
        sel4_call(
            fs_ep,
            msg_info(FS_OP_INIT, 0, 0, 2),
            &[XV6_HOST_TO_FS_PROTOCOL, XV6_ABI_VERSION],
        )
    };
    let label = msg_label(reply.info);
    if label != 0 || reply.mrs[0] != XV6_OK {
        log("xv6-host: fs init failed label=");
        print_u64(label);
        log(" status=");
        print_u64(reply.mrs[0]);
        log("\n");
        halt_loop();
    }
    if reply.mrs[1] != VIRTIO_BLK_SECTOR_SIZE as u64 || reply.mrs[2] == 0 {
        log("xv6-host: fs init returned invalid geometry\n");
        halt_loop();
    }
    log("xv6-host: fs server ready sector=");
    print_u64(reply.mrs[1]);
    log(" block=");
    print_u64(reply.mrs[2]);
    log(" disk-blocks=");
    print_u64(reply.mrs[3]);
    log("\n");
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
    log("xv6-host: panic\n");
    halt_loop()
}
