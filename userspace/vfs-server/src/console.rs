use core::cmp::min;
use core::sync::atomic::{Ordering, fence};

use sel4_user::{msg_info, msg_label, sel4_call};
use xv6_abi::{
    UartOp, XV6_ABI_VERSION, XV6_UART_ENDPOINT_CPTR, Xv6FileType, Xv6Protocol, Xv6Status,
};

use crate::ipc::{with_shared_buffer, with_shared_buffer_mut};
use crate::state::{CONSOLE_BUF_SIZE, with_console, with_console_mut};

const CONSOLE_INPUT: &[u8] = match option_env!("XV6_CONSOLE_INPUT") {
    Some(input) => input.as_bytes(),
    None => b"",
};
const CTRL_D: u8 = 0x04;
const CTRL_H: u8 = 0x08;
const CTRL_U: u8 = 0x15;
const DEL: u8 = 0x7f;

pub(crate) fn init_console() -> bool {
    let reply = uart_call(
        UartOp::Init.raw(),
        &[Xv6Protocol::VfsToUart.raw(), XV6_ABI_VERSION],
    );
    reply[0] == Xv6Status::Ok.raw()
}

pub(crate) async fn read_console(max_len: usize) -> [u64; 4] {
    if max_len == 0 {
        return [Xv6Status::Ok.raw(), 0, Xv6FileType::Device.raw() as u64, 0];
    }
    if !CONSOLE_INPUT.is_empty() {
        drain_scripted_input().await;
    } else if let Err(reply) = drain_uart_input().await {
        return reply;
    }

    let (n, eof) = copy_console_output(max_len);
    if n == 0 {
        if eof || (!CONSOLE_INPUT.is_empty() && scripted_input_done()) {
            return [Xv6Status::Ok.raw(), 0, Xv6FileType::Device.raw() as u64, 0];
        }
        return [Xv6Status::WouldBlock.raw(), 0, 0, 0];
    }
    fence(Ordering::SeqCst);
    [
        Xv6Status::Ok.raw(),
        n as u64,
        Xv6FileType::Device.raw() as u64,
        0,
    ]
}

async fn drain_scripted_input() {
    while !console_has_complete_line() && !scripted_input_done() {
        let ch = with_console_mut(|console| {
            let ch = CONSOLE_INPUT[console.input_pos];
            console.input_pos += 1;
            ch
        });
        handle_console_input(ch).await;
    }
    if scripted_input_done() && !console_has_complete_line() {
        publish_partial_scripted_line();
    }
}

async fn drain_uart_input() -> Result<(), [u64; 4]> {
    while !console_has_complete_line() {
        let reply = uart_getch().await;
        if reply[0] == Xv6Status::WouldBlock.raw() {
            break;
        }
        if reply[0] != Xv6Status::Ok.raw() {
            return Err([Xv6Status::InvalidArgument.raw(), 0, 0, 0]);
        }
        handle_console_input(reply[1] as u8).await;
    }
    Ok(())
}

async fn handle_console_input(mut ch: u8) {
    if ch == b'\r' {
        ch = b'\n';
    }
    match ch {
        CTRL_U => {
            while can_erase_console_char() {
                erase_console_char();
                echo_backspace().await;
            }
        }
        CTRL_H | DEL => {
            if can_erase_console_char() {
                erase_console_char();
                echo_backspace().await;
            }
        }
        _ => {
            if ch != 0 && console_input_room() {
                push_console_char(ch);
                let _ = uart_putch(ch).await;
            }
        }
    }
}

fn console_has_complete_line() -> bool {
    with_console(|console| console.r != console.w)
}

fn scripted_input_done() -> bool {
    with_console(|console| console.input_pos >= CONSOLE_INPUT.len())
}

fn console_input_room() -> bool {
    with_console(|console| console.e.wrapping_sub(console.r) < CONSOLE_BUF_SIZE)
}

fn can_erase_console_char() -> bool {
    with_console(|console| console.e != console.w)
}

fn erase_console_char() {
    with_console_mut(|console| {
        console.e = console.e.wrapping_sub(1);
    });
}

fn push_console_char(ch: u8) {
    with_console_mut(|console| {
        console.buf[console.e % CONSOLE_BUF_SIZE] = ch;
        console.e = console.e.wrapping_add(1);
        if ch == b'\n' || ch == CTRL_D || console.e.wrapping_sub(console.r) == CONSOLE_BUF_SIZE {
            console.w = console.e;
        }
    });
}

fn copy_console_output(max_len: usize) -> (usize, bool) {
    with_shared_buffer_mut(|dst| {
        with_console_mut(|console| {
            let mut n = 0usize;
            let mut eof = false;
            while n < max_len && console.r != console.w {
                let ch = console.buf[console.r % CONSOLE_BUF_SIZE];
                if ch == CTRL_D {
                    if n > 0 {
                        break;
                    }
                    console.r = console.r.wrapping_add(1);
                    eof = true;
                    break;
                }
                dst[n] = ch;
                console.r = console.r.wrapping_add(1);
                n += 1;
                if ch == b'\n' {
                    break;
                }
            }
            (n, eof)
        })
    })
}

fn publish_partial_scripted_line() {
    with_console_mut(|console| {
        if console.e != console.r {
            console.w = console.e;
        }
    });
}

async fn echo_backspace() {
    let _ = uart_putch(CTRL_H).await;
    let _ = uart_putch(b' ').await;
    let _ = uart_putch(CTRL_H).await;
}

pub(crate) async fn write_console(max_len: usize) -> [u64; 4] {
    let len = min(max_len, xv6_abi::XV6_MAX_FILE_WRITE);
    let mut done = 0usize;
    while done < len {
        let mut bytes = [0u8; 64];
        let chunk = min(len - done, bytes.len());
        with_shared_buffer(|src| {
            bytes[..chunk].copy_from_slice(&src[done..done + chunk]);
        });
        let mut i = 0usize;
        while i < chunk {
            if !uart_putch(bytes[i]).await {
                return [
                    Xv6Status::InvalidArgument.raw(),
                    (done + i) as u64,
                    Xv6FileType::Device.raw() as u64,
                    0,
                ];
            }
            i += 1;
        }
        done += chunk;
    }
    [
        Xv6Status::Ok.raw(),
        len as u64,
        Xv6FileType::Device.raw() as u64,
        0,
    ]
}

async fn uart_putch(ch: u8) -> bool {
    let reply = uart_call(
        UartOp::PutChar.raw(),
        &[Xv6Protocol::VfsToUart.raw(), XV6_ABI_VERSION, ch as u64],
    );
    reply[0] == Xv6Status::Ok.raw()
}

async fn uart_getch() -> [u64; 4] {
    uart_call(
        UartOp::GetChar.raw(),
        &[Xv6Protocol::VfsToUart.raw(), XV6_ABI_VERSION],
    )
}

fn uart_call(label: u64, mrs: &[u64]) -> [u64; 4] {
    let reply = unsafe {
        sel4_call(
            XV6_UART_ENDPOINT_CPTR,
            msg_info(label, 0, 0, mrs.len() as u64),
            mrs,
        )
    };
    if msg_label(reply.info) != 0 {
        return [Xv6Status::InvalidArgument.raw(), 0, 0, 0];
    }
    [reply.mrs[0], reply.mrs[1], reply.mrs[2], reply.mrs[3]]
}
