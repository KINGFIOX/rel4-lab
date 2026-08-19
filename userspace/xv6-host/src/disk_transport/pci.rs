use sel4_user::{call_checked, cap_rights};

// Staged pc99 PCI windows. Wired when the x86_64 userspace backend lands.
const X86_64_PCIE_ECAM_BASE: u64 = 0xe000_0000;
const X86_64_PCIE_IO_BASE: u64 = 0;
const X86_64_PCIE_MEM_BASE: u64 = 0x8000_0000;
const X86_64_PCIE_LEGACY_IRQ_BASE: u64 = 16;
const X86_64_PCH_MSI_BASE: u64 = 0xfee0_0000;
const XV6_PCIE_ECAM_VADDR: u64 = 0x5001_0000;
const XV6_PCIE_ECAM_MAP_SIZE: u64 = 0x0010_0000;
const XV6_PCIE_MEM_VADDR: u64 = 0x5011_0000;
const XV6_PCIE_MEM_MAP_SIZE: u64 = 0x0001_0000;
const XV6_PCIE_IO_VADDR: u64 = 0x5012_0000;
const XV6_PCIE_IO_MAP_SIZE: u64 = 0x0001_0000;
const XV6_PCIE_MSI_VADDR: u64 = 0x5013_0000;
const XV6_PCIE_MSI_MAP_SIZE: u64 = 0x1000;

use crate::allocator::Allocator;
use crate::consts::{
    IRQ_CONTROL, LABEL_IRQ_ISSUE_IRQ_HANDLER, LABEL_IRQ_SET_NOTIFICATION, PAGE_SIZE, ROOT_CNODE,
    ROOT_CNODE_DEPTH, Xv6Badge,
};
use crate::disk_transport::{FrameMap, push_frame_map};
use crate::util::{halt_loop, warn};

const fn pages(size: u64) -> usize {
    ((size + PAGE_SIZE - 1) / PAGE_SIZE) as usize
}

pub(crate) const MAX_DEVICE_FRAME_MAPS: usize = pages(XV6_PCIE_ECAM_MAP_SIZE)
    + pages(XV6_PCIE_MEM_MAP_SIZE)
    + pages(XV6_PCIE_IO_MAP_SIZE)
    + pages(XV6_PCIE_MSI_MAP_SIZE);

pub(crate) fn issue_irq_handler(alloc: &mut Allocator, disk_irq_ntfn: u64) -> u64 {
    let disk_irq_handler = alloc.alloc_slot();
    call_checked(
        IRQ_CONTROL,
        LABEL_IRQ_ISSUE_IRQ_HANDLER,
        &[ROOT_CNODE],
        &[
            X86_64_PCIE_LEGACY_IRQ_BASE,
            disk_irq_handler,
            ROOT_CNODE_DEPTH,
        ],
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

pub(crate) fn append_device_frame_maps(
    alloc: &mut Allocator,
    maps: &mut [FrameMap],
    len: &mut usize,
) {
    append_device_window(
        alloc,
        maps,
        len,
        X86_64_PCIE_ECAM_BASE,
        XV6_PCIE_ECAM_VADDR,
        XV6_PCIE_ECAM_MAP_SIZE,
    );
    append_device_window(
        alloc,
        maps,
        len,
        X86_64_PCIE_MEM_BASE,
        XV6_PCIE_MEM_VADDR,
        XV6_PCIE_MEM_MAP_SIZE,
    );
    append_device_window(
        alloc,
        maps,
        len,
        X86_64_PCIE_IO_BASE,
        XV6_PCIE_IO_VADDR,
        XV6_PCIE_IO_MAP_SIZE,
    );
    append_device_window(
        alloc,
        maps,
        len,
        X86_64_PCH_MSI_BASE,
        XV6_PCIE_MSI_VADDR,
        XV6_PCIE_MSI_MAP_SIZE,
    );
}

fn append_device_window(
    alloc: &mut Allocator,
    maps: &mut [FrameMap],
    len: &mut usize,
    paddr: u64,
    vaddr: u64,
    size: u64,
) {
    if paddr & (PAGE_SIZE - 1) != 0 || vaddr & (PAGE_SIZE - 1) != 0 || size & (PAGE_SIZE - 1) != 0 {
        warn!("xv6-host: unaligned PCI device window");
        halt_loop();
    }
    let mut offset = 0u64;
    while offset < size {
        let frame = alloc.retype_device_4k_at(paddr + offset);
        if !push_frame_map(maps, len, (frame, vaddr + offset, true, false, 0)) {
            warn!("xv6-host: disk frame map table exhausted");
            halt_loop();
        }
        offset += PAGE_SIZE;
    }
}
