#![no_std]
#![no_main]
#![deny(unsafe_attr_outside_unsafe)]
#![deny(unsafe_op_in_unsafe_fn)]

use core::panic::PanicInfo;

mod completion;
mod device;
mod layout;
mod request;
mod types;

use sel4_user::{
    IpcMessage, error, halt_loop, info, init_ipc_buffer, init_logger, msg_info, msg_label, rt, warn,
};
use types::RequestResult;
use xv6_abi::{
    DiskRequestOp, VIRTIO_BLK_SECTOR_SIZE, XV6_ABI_VERSION, XV6_SERVER_RECV_REPLY_CPTR,
    XV6_SERVICE_ENDPOINT_CPTR, Xv6Protocol,
};

#[unsafe(no_mangle)]
pub extern "C" fn _start(ipc_buffer: usize, dma_paddr: usize) -> ! {
    init_ipc_buffer(ipc_buffer as u64);
    init_logger();
    request::init();
    device::init(dma_paddr as u64);
    info!(
        "virtio-disk-server: boot protocol={} abi={} sector={} first-op={}",
        Xv6Protocol::FsToDisk.raw(),
        XV6_ABI_VERSION,
        VIRTIO_BLK_SECTOR_SIZE,
        DiskRequestOp::GetInfo.raw()
    );
    info!("virtio-disk-server: waiting for fs-server client hookup");
    rt::block_on(server_loop());
    error!("virtio-disk-server: server loop returned");
    halt_loop()
}

async fn server_loop() {
    let mut state = ServerState::new();
    loop {
        let msg = recv_next_message(&mut state).await;
        handle_message(&mut state, &msg).await;
    }
}

struct ServerState {
    reply_pending: bool,
    reply_mrs: [u64; 4],
}

impl ServerState {
    const fn new() -> Self {
        Self {
            reply_pending: false,
            reply_mrs: [0; 4],
        }
    }

    fn stage_reply(&mut self, mrs: [u64; 4]) {
        self.reply_mrs = mrs;
        self.reply_pending = true;
    }
}

async fn recv_next_message(state: &mut ServerState) -> IpcMessage {
    if state.reply_pending {
        state.reply_pending = false;
        rt::reply_recv_with_reply(
            XV6_SERVICE_ENDPOINT_CPTR,
            msg_info(0, 0, 0, state.reply_mrs.len() as u64),
            &state.reply_mrs,
            XV6_SERVER_RECV_REPLY_CPTR,
        )
        .await
    } else {
        rt::recv_with_reply(XV6_SERVICE_ENDPOINT_CPTR, XV6_SERVER_RECV_REPLY_CPTR).await
    }
}

async fn handle_message(state: &mut ServerState, msg: &IpcMessage) {
    if request::is_disk_irq(msg) {
        request::handle_disk_irq();
        return;
    }
    if msg_label(msg.info) == 0 {
        warn!(
            "virtio-disk-server: unexpected zero-label IPC badge={:#x}",
            msg.badge
        );
        return;
    }

    match request::handle(msg).await {
        RequestResult::Reply(mrs) => {
            state.stage_reply(mrs);
        }
        RequestResult::Deferred => {}
    }
}

#[panic_handler]
fn panic(_: &PanicInfo) -> ! {
    error!("virtio-disk-server: panic");
    halt_loop()
}
