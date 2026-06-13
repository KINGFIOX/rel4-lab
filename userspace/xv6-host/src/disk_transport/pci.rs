compile_error!(
    "xv6-host LoongArch64 disk transport requires virtio-pci capability mapping and interrupt routing for QEMU virt"
);

use crate::allocator::Allocator;
use crate::disk_transport::FrameMap;

pub(crate) fn issue_irq_handler(_alloc: &mut Allocator, _disk_irq_ntfn: u64) -> u64 {
    0
}

pub(crate) fn device_frame_map(_alloc: &mut Allocator) -> Option<FrameMap> {
    None
}
