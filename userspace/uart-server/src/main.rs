#![no_std]
#![no_main]
#![deny(unsafe_attr_outside_unsafe)]
#![deny(unsafe_op_in_unsafe_fn)]

use core::panic::PanicInfo;
use core::ptr;
use core::ptr::{read_volatile, write_volatile};

use sel4_user::{
    IpcMessage, error, halt_loop, info, init_ipc_buffer, init_logger, msg_info, msg_label,
    sel4_recv_with_reply, sel4_reply_recv_with_reply, sel4_send,
};
use xv6_abi::{
    UartOp, XV6_ABI_VERSION, XV6_SERVER_RECV_REPLY_CPTR, XV6_SERVICE_ENDPOINT_CPTR,
    XV6_UART_MMIO_VADDR, XV6_UART_REPLY_ENDPOINT_CPTR, Xv6Protocol, Xv6Status,
};

const RHR: usize = 0;
const THR: usize = 0;
const IER: usize = 1;
const FCR: usize = 2;
const LCR: usize = 3;
const LSR: usize = 5;
const LSR_DR: u8 = 1 << 0;
const LSR_THRE: u8 = 1 << 5;

#[unsafe(no_mangle)]
pub extern "C" fn _start(ipc_buffer: usize) -> ! {
    unsafe {
        clear_bss();
    }
    init_ipc_buffer(ipc_buffer as u64);
    init_logger();
    info!("uart-server: boot");

    let mut reply_pending = false;
    let mut reply_mrs = [0u64; 4];
    loop {
        let msg = if reply_pending {
            unsafe {
                sel4_reply_recv_with_reply(
                    XV6_SERVICE_ENDPOINT_CPTR,
                    msg_info(0, 0, 0, 4),
                    &reply_mrs,
                    XV6_SERVER_RECV_REPLY_CPTR,
                )
            }
        } else {
            unsafe { sel4_recv_with_reply(XV6_SERVICE_ENDPOINT_CPTR, XV6_SERVER_RECV_REPLY_CPTR) }
        };
        if is_async_request(&msg) {
            handle_async_request(&msg);
            reply_pending = false;
        } else {
            reply_mrs = handle_request(&msg);
            reply_pending = true;
        }
    }
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

fn handle_request(msg: &IpcMessage) -> [u64; 4] {
    if !valid_request(msg) {
        return [Xv6Status::InvalidArgument.raw(), 0, 0, 0];
    }
    match UartOp::from_raw(msg_label(msg.info)) {
        Some(UartOp::Init) => {
            init_uart();
            [Xv6Status::Ok.raw(), 0, 0, 0]
        }
        Some(UartOp::PutChar) => {
            putch(msg.mrs[2] as u8);
            [Xv6Status::Ok.raw(), 1, 0, 0]
        }
        Some(UartOp::GetChar) => {
            let ch = getch();
            if ch < 0 {
                [Xv6Status::WouldBlock.raw(), 0, 0, 0]
            } else {
                [Xv6Status::Ok.raw(), ch as u64, 0, 0]
            }
        }
        None => [Xv6Status::InvalidArgument.raw(), 0, 0, 0],
    }
}

fn valid_request(msg: &IpcMessage) -> bool {
    (msg.mrs[0] == Xv6Protocol::VfsToUart.raw() && msg.mrs[1] == XV6_ABI_VERSION)
        || is_async_request(msg)
}

fn is_async_request(msg: &IpcMessage) -> bool {
    msg.mrs[0] == Xv6Protocol::VfsToUartAsync.raw() && msg.mrs[1] != 0
}

fn handle_async_request(msg: &IpcMessage) {
    let request_id = msg.mrs[1];
    let reply = handle_request(msg);
    unsafe {
        sel4_send(
            XV6_UART_REPLY_ENDPOINT_CPTR,
            msg_info(Xv6Protocol::VfsToUartAsync.raw(), 0, 0, 5),
            &[request_id, reply[0], reply[1], reply[2], reply[3]],
        );
    }
}

fn init_uart() {
    unsafe {
        write_volatile(reg(IER), 0x00);
        write_volatile(reg(FCR), 0x01);
        write_volatile(reg(LCR), 0x03);
    }
    info!("uart-server: init complete");
}

fn putch(ch: u8) {
    unsafe {
        while read_volatile(reg(LSR)) & LSR_THRE == 0 {}
        write_volatile(reg(THR), ch);
    }
}

fn getch() -> i32 {
    unsafe {
        if read_volatile(reg(LSR)) & LSR_DR == 0 {
            -1
        } else {
            read_volatile(reg(RHR)) as i32
        }
    }
}

#[inline]
fn reg(offset: usize) -> *mut u8 {
    (XV6_UART_MMIO_VADDR as usize + offset) as *mut u8
}

#[panic_handler]
fn panic(_: &PanicInfo) -> ! {
    error!("uart-server: panic");
    halt_loop()
}
