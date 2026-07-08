#![no_std]
#![no_main]
#![deny(unsafe_attr_outside_unsafe)]
#![deny(unsafe_op_in_unsafe_fn)]

mod console;
mod ipc;
mod ops;
mod pipe;
mod state;

use core::panic::PanicInfo;

use sel4_user::{error, halt_loop, info, init_ipc_buffer, init_logger, msg_info, rt};
use xv6_abi::{XV6_SERVER_RECV_REPLY_CPTR, XV6_SERVICE_ENDPOINT_CPTR};

#[unsafe(no_mangle)]
pub extern "C" fn _start(ipc_buffer: usize) -> ! {
    init_ipc_buffer(ipc_buffer as u64);
    init_logger();
    info!("vfs-server: boot");
    rt::block_on(server_loop());
    error!("vfs-server: server loop returned");
    halt_loop()
}

async fn server_loop() {
    let mut reply_pending = false;
    let mut reply_mrs = [0u64; 4];
    loop {
        let msg = if reply_pending {
            rt::reply_recv_with_reply(
                XV6_SERVICE_ENDPOINT_CPTR,
                msg_info(0, 0, 0, 4),
                &reply_mrs,
                XV6_SERVER_RECV_REPLY_CPTR,
            )
            .await
        } else {
            rt::recv_with_reply(XV6_SERVICE_ENDPOINT_CPTR, XV6_SERVER_RECV_REPLY_CPTR).await
        };
        match ops::handle_request(&msg).await {
            ops::RequestResult::Reply(mrs) => {
                reply_mrs = mrs;
                reply_pending = true;
            }
            ops::RequestResult::Deferred => {
                reply_pending = false;
            }
        }
    }
}

#[panic_handler]
fn panic(_: &PanicInfo) -> ! {
    error!("vfs-server: panic");
    halt_loop()
}
