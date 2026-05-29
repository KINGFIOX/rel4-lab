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
use child::{create_child, load_payload, map_stack, start_child};
use consts::{FAULT_UNKNOWN_SYSCALL, LABEL_IRQ_ISSUE_IRQ_HANDLER, LABEL_IRQ_SET_NOTIFICATION};
use consts::{INIT_TCB, IRQ_CONTROL, KERNEL_TIMER_IRQ, OBJ_NOTIFICATION, ROOT_CNODE};
use consts::{LABEL_TCB_BIND_NOTIFICATION, ROOT_CNODE_DEPTH};
use sel4::{call_checked, init_ipc_buffer, msg_info, msg_label, sel4_recv, sel4_reply_recv};
use types::BootInfo;
use util::{halt_loop, log, print_u64};
use xv6::handle_xv6_syscall;

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
    xv6::init_fds();
    let mut child = create_child(&mut alloc);
    setup_timer_notification(&mut alloc);
    load_payload(&mut alloc, &mut child);
    map_stack(&mut alloc, &child);
    start_child(&child);

    log("xv6-host: waiting for fault IPC\n");
    let mut reply_pending = false;
    let mut reply_mrs = [0u64; 11];
    loop {
        let msg = if reply_pending {
            reply_pending = false;
            unsafe { sel4_reply_recv(child.fault_ep, msg_info(0, 0, 0, 11), &reply_mrs) }
        } else {
            unsafe { sel4_recv(child.fault_ep) }
        };

        let label = msg_label(msg.info);
        if label == 0 {
            xv6::tick();
            continue;
        }
        if label != FAULT_UNKNOWN_SYSCALL {
            log("xv6-host: non-syscall fault label=");
            print_u64(label);
            log("\n");
            halt_loop();
        }

        unsafe {
            if !SAW_FAULT_IPC {
                SAW_FAULT_IPC = true;
                log("xv6-host: UnknownSyscall fault IPC\n");
            }
        }

        reply_mrs = msg.mrs[..11].try_into().unwrap_or([0; 11]);
        let ret = handle_xv6_syscall(&mut alloc, &mut child, &msg.mrs);
        reply_mrs[0] = msg.mrs[0].wrapping_add(4);
        reply_mrs[3] = ret as u64;
        reply_pending = true;
    }
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
