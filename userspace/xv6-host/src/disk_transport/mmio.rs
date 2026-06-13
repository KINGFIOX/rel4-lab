use sel4_user::{call_checked, cap_rights};
use xv6_abi::platform::current::{
    VIRTIO_MMIO_FRAME_BASE, VIRTIO0_IRQ, XV6_VIRTIO_MMIO_FRAME_VADDR,
};

use crate::allocator::Allocator;
use crate::consts::{
    IRQ_CONTROL, LABEL_IRQ_ISSUE_IRQ_HANDLER, LABEL_IRQ_SET_NOTIFICATION, ROOT_CNODE,
    ROOT_CNODE_DEPTH, Xv6Badge,
};
use crate::disk_transport::FrameMap;

pub(crate) fn issue_irq_handler(alloc: &mut Allocator, disk_irq_ntfn: u64) -> u64 {
    let disk_irq_handler = alloc.alloc_slot();
    call_checked(
        IRQ_CONTROL,
        LABEL_IRQ_ISSUE_IRQ_HANDLER,
        &[ROOT_CNODE],
        &[VIRTIO0_IRQ, disk_irq_handler, ROOT_CNODE_DEPTH],
    );
    let disk_irq_signal_cap = alloc.mint_cap(
        disk_irq_ntfn,
        cap_rights(false, false, false, true),
        Xv6Badge::DiskIrq.raw(),
    );
    call_checked(
        disk_irq_handler,
        LABEL_IRQ_SET_NOTIFICATION,
        &[disk_irq_signal_cap],
        &[],
    );
    disk_irq_handler
}

pub(crate) fn device_frame_map(alloc: &mut Allocator) -> Option<FrameMap> {
    let virtio_mmio_frame = alloc.retype_device_4k_at(VIRTIO_MMIO_FRAME_BASE);
    Some((virtio_mmio_frame, XV6_VIRTIO_MMIO_FRAME_VADDR, true, false))
}
