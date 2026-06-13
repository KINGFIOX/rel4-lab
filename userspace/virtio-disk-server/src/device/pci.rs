use core::ptr;
use core::sync::atomic::{AtomicBool, AtomicU16, AtomicU32, AtomicU64, Ordering, fence};

use sel4_user::{LABEL_IRQ_ACK, info, msg_info, msg_label, sel4_call, warn};
use xv6_abi::platform::current::{
    LOONGARCH64_PCIE_MEM_BASE, XV6_PCIE_ECAM_MAP_SIZE, XV6_PCIE_ECAM_VADDR, XV6_PCIE_IO_MAP_SIZE,
    XV6_PCIE_IO_VADDR, XV6_PCIE_MEM_MAP_SIZE, XV6_PCIE_MEM_VADDR,
};
use xv6_abi::{
    FS_BLOCK_SIZE, VIRTIO_BLK_DEVICE_ID, VIRTIO_BLK_F_FLUSH, VIRTIO_BLK_SECTOR_SIZE,
    VIRTIO_BLK_T_FLUSH, VIRTIO_CONFIG_S_ACKNOWLEDGE, VIRTIO_CONFIG_S_DRIVER,
    VIRTIO_CONFIG_S_DRIVER_OK, VIRTIO_CONFIG_S_FEATURES_OK, VIRTIO_QUEUE_NUM, VIRTQ_DESC_F_NEXT,
    VIRTQ_DESC_F_WRITE, XV6_DISK_IRQ_HANDLER_CPTR, XV6_DISK_MAX_IN_FLIGHT, XV6_VIRTIO_DMA_VADDR,
};

use crate::layout::{
    AVAIL_OFF, DESC_OFF, DESCS_PER_REQUEST, USED_OFF, data_off, desc_head, dma_va, req_off,
    shared_buffer_va, status_off,
};

const PCI_DEVICE_STRIDE: u64 = 1 << 15;
const PCI_FUNCTION_STRIDE: u64 = 1 << 12;
const PCI_FUNCTIONS_PER_DEVICE: u64 = 8;
const PCI_MAX_DEVICES: u64 = 32;
const PCI_MAX_CAPS: usize = 64;
const PCI_VENDOR_INVALID: u16 = 0xffff;
const PCI_VENDOR_VIRTIO: u16 = 0x1af4;
const PCI_DEVICE_VIRTIO_BLK_MODERN: u16 = 0x1040 + VIRTIO_BLK_DEVICE_ID as u16;
const PCI_CAP_ID_VNDR: u8 = 0x09;

const PCI_VENDOR_ID: u64 = 0x00;
const PCI_DEVICE_ID: u64 = 0x02;
const PCI_COMMAND: u64 = 0x04;
const PCI_STATUS: u64 = 0x06;
const PCI_HEADER_TYPE: u64 = 0x0e;
const PCI_BAR0: u64 = 0x10;
const PCI_CAP_PTR: u64 = 0x34;
const PCI_STATUS_CAP_LIST: u16 = 1 << 4;
const PCI_COMMAND_IO_SPACE: u16 = 1 << 0;
const PCI_COMMAND_MEMORY_SPACE: u16 = 1 << 1;
const PCI_COMMAND_BUS_MASTER: u16 = 1 << 2;
const PCI_COMMAND_INTX_DISABLE: u16 = 1 << 10;
const PCI_BAR_IO: u32 = 1 << 0;
const PCI_BAR_MEM_TYPE_MASK: u32 = 0x6;
const PCI_BAR_MEM_TYPE_64: u32 = 0x4;

const VIRTIO_PCI_CAP_COMMON_CFG: u8 = 1;
const VIRTIO_PCI_CAP_NOTIFY_CFG: u8 = 2;
const VIRTIO_PCI_CAP_ISR_CFG: u8 = 3;
const VIRTIO_PCI_CAP_DEVICE_CFG: u8 = 4;
const VIRTIO_PCI_CAP_PCI_CFG: u8 = 5;
const VIRTIO_PCI_CAP_BASE_LEN: u8 = 16;
const VIRTIO_PCI_NOTIFY_CAP_LEN: u8 = 20;

const VIRTIO_PCI_DEVICE_FEATURE_SELECT: u64 = 0x00;
const VIRTIO_PCI_DEVICE_FEATURE: u64 = 0x04;
const VIRTIO_PCI_DRIVER_FEATURE_SELECT: u64 = 0x08;
const VIRTIO_PCI_DRIVER_FEATURE: u64 = 0x0c;
const VIRTIO_PCI_DEVICE_STATUS: u64 = 0x14;
const VIRTIO_PCI_QUEUE_SELECT: u64 = 0x16;
const VIRTIO_PCI_QUEUE_SIZE: u64 = 0x18;
const VIRTIO_PCI_QUEUE_ENABLE: u64 = 0x1c;
const VIRTIO_PCI_QUEUE_NOTIFY_OFF: u64 = 0x1e;
const VIRTIO_PCI_QUEUE_DESC: u64 = 0x20;
const VIRTIO_PCI_QUEUE_DRIVER: u64 = 0x28;
const VIRTIO_PCI_QUEUE_DEVICE: u64 = 0x30;
const VIRTIO_F_VERSION_1: u64 = 1 << 32;

const LOONGARCH64_PCIE_IO_PORT_BASE: u64 = 0x4000;

static DMA_PADDR: AtomicU64 = AtomicU64::new(0);
static USED_IDX: AtomicU16 = AtomicU16::new(0);
static DISK_READY: AtomicBool = AtomicBool::new(false);
static DISK_CAN_FLUSH: AtomicBool = AtomicBool::new(false);
static COMMON_CFG_VADDR: AtomicU64 = AtomicU64::new(0);
static NOTIFY_CFG_VADDR: AtomicU64 = AtomicU64::new(0);
static NOTIFY_OFF_MULTIPLIER: AtomicU32 = AtomicU32::new(0);
static QUEUE_NOTIFY_OFF: AtomicU16 = AtomicU16::new(0);
static ISR_CFG_VADDR: AtomicU64 = AtomicU64::new(0);

pub fn init(dma_paddr: u64) -> bool {
    DMA_PADDR.store(dma_paddr, Ordering::Relaxed);
    let ready = init_virtio_disk();
    DISK_READY.store(ready, Ordering::Relaxed);
    ready
}

pub fn ready() -> bool {
    DISK_READY.load(Ordering::Relaxed)
}

pub fn can_flush() -> bool {
    DISK_CAN_FLUSH.load(Ordering::Relaxed)
}

pub fn copy_shared_to_dma(request_slot: usize, shared_slot: u64) {
    fence(Ordering::SeqCst);
    unsafe {
        ptr::copy_nonoverlapping(
            shared_buffer_va(shared_slot) as *const u8,
            dma_va(data_off(request_slot)) as *mut u8,
            FS_BLOCK_SIZE,
        );
    }
    fence(Ordering::SeqCst);
}

pub fn copy_dma_to_shared(request_slot: usize, shared_slot: u64) {
    unsafe {
        ptr::copy_nonoverlapping(
            dma_va(data_off(request_slot)) as *const u8,
            shared_buffer_va(shared_slot) as *mut u8,
            FS_BLOCK_SIZE,
        );
    }
    fence(Ordering::SeqCst);
}

pub fn request_status(request_slot: usize) -> u8 {
    unsafe { ptr::read_volatile(dma_va(status_off(request_slot)) as *const u8) }
}

pub fn prepare_block_descriptor(
    request_slot: usize,
    blockno: u64,
    request_type: u32,
    data_writable_by_device: bool,
) {
    let sector = blockno * (FS_BLOCK_SIZE / VIRTIO_BLK_SECTOR_SIZE) as u64;
    let head = desc_head(request_slot);
    write32(req_off(request_slot), request_type);
    write32(req_off(request_slot) + 4, 0);
    write64(req_off(request_slot) + 8, sector);

    write_desc(
        head,
        dma_pa(req_off(request_slot)),
        16,
        VIRTQ_DESC_F_NEXT,
        head + 1,
    );
    let data_flags = if data_writable_by_device {
        VIRTQ_DESC_F_WRITE | VIRTQ_DESC_F_NEXT
    } else {
        VIRTQ_DESC_F_NEXT
    };
    write_desc(
        head + 1,
        dma_pa(data_off(request_slot)),
        FS_BLOCK_SIZE as u32,
        data_flags,
        head + 2,
    );
    unsafe {
        ptr::write_volatile(dma_va(status_off(request_slot)) as *mut u8, 0xff);
    }
    write_desc(
        head + 2,
        dma_pa(status_off(request_slot)),
        1,
        VIRTQ_DESC_F_WRITE,
        0,
    );
}

pub fn prepare_flush_descriptor(request_slot: usize) {
    let head = desc_head(request_slot);
    write32(req_off(request_slot), VIRTIO_BLK_T_FLUSH);
    write32(req_off(request_slot) + 4, 0);
    write64(req_off(request_slot) + 8, 0);

    write_desc(
        head,
        dma_pa(req_off(request_slot)),
        16,
        VIRTQ_DESC_F_NEXT,
        head + 1,
    );
    unsafe {
        ptr::write_volatile(dma_va(status_off(request_slot)) as *mut u8, 0xff);
    }
    write_desc(
        head + 1,
        dma_pa(status_off(request_slot)),
        1,
        VIRTQ_DESC_F_WRITE,
        0,
    );
}

pub fn publish_request(request_slot: usize) {
    let head = desc_head(request_slot);
    let avail_idx = read16(AVAIL_OFF + 2);
    write16(
        AVAIL_OFF + 4 + ((avail_idx as u64 % VIRTIO_QUEUE_NUM as u64) * 2),
        head,
    );
    fence(Ordering::SeqCst);
    write16(AVAIL_OFF + 2, avail_idx.wrapping_add(1));
    fence(Ordering::SeqCst);

    pci_notify_queue();
}

pub fn begin_used_drain() {
    fence(Ordering::SeqCst);
}

pub fn next_used_head() -> Option<u16> {
    let used_idx = read16(USED_OFF + 2);
    let next_used = USED_IDX.load(Ordering::Relaxed);
    if next_used == used_idx {
        return None;
    }
    let ring_index = next_used as u64 % VIRTIO_QUEUE_NUM as u64;
    let head = read32(USED_OFF + 4 + ring_index * 8) as u16;
    USED_IDX.store(next_used.wrapping_add(1), Ordering::Relaxed);
    Some(head)
}

pub fn end_used_drain() {
    fence(Ordering::SeqCst);
}

pub fn ack_irq_handler() -> bool {
    let reply = unsafe {
        sel4_call(
            XV6_DISK_IRQ_HANDLER_CPTR,
            msg_info(LABEL_IRQ_ACK, 0, 0, 0),
            &[],
        )
    };
    if msg_label(reply.info) != 0 {
        warn!(
            "virtio-disk-server: irq ack failed label={}",
            msg_label(reply.info)
        );
        return false;
    }
    true
}

pub fn ack_virtio_interrupt() {
    let isr = ISR_CFG_VADDR.load(Ordering::Relaxed);
    if isr != 0 {
        let _ = unsafe { ptr::read_volatile(isr as *const u8) };
    }
}

fn init_virtio_disk() -> bool {
    let Some(device) = discover_virtio_blk() else {
        warn!("virtio-disk-server: virtio-pci block device not found");
        return false;
    };

    COMMON_CFG_VADDR.store(device.common_cfg, Ordering::Relaxed);
    NOTIFY_CFG_VADDR.store(device.notify_cfg, Ordering::Relaxed);
    NOTIFY_OFF_MULTIPLIER.store(device.notify_off_multiplier, Ordering::Relaxed);
    ISR_CFG_VADDR.store(device.isr_cfg, Ordering::Relaxed);
    enable_pci_function(device.function);

    write_device_status(0);
    if read_device_status() != 0 {
        warn!("virtio-disk-server: virtio-pci reset did not complete");
        return false;
    }

    let mut status = 0u32;
    status |= VIRTIO_CONFIG_S_ACKNOWLEDGE;
    write_device_status(status);
    status |= VIRTIO_CONFIG_S_DRIVER;
    write_device_status(status);

    let device_features = read_device_features();
    let driver_features = device_features & (VIRTIO_F_VERSION_1 | feature_bit(VIRTIO_BLK_F_FLUSH));
    if (driver_features & VIRTIO_F_VERSION_1) == 0 {
        warn!("virtio-disk-server: virtio-pci device lacks VERSION_1");
        return false;
    }
    write_driver_features(driver_features);

    status |= VIRTIO_CONFIG_S_FEATURES_OK;
    write_device_status(status);
    if (read_device_status() & VIRTIO_CONFIG_S_FEATURES_OK) == 0 {
        warn!("virtio-disk-server: FEATURES_OK rejected");
        return false;
    }
    DISK_CAN_FLUSH.store(
        (driver_features & feature_bit(VIRTIO_BLK_F_FLUSH)) != 0,
        Ordering::Relaxed,
    );

    common_write16(VIRTIO_PCI_QUEUE_SELECT, 0);
    if common_read16(VIRTIO_PCI_QUEUE_ENABLE) != 0 {
        warn!("virtio-disk-server: queue already enabled");
        return false;
    }
    let queue_max = common_read16(VIRTIO_PCI_QUEUE_SIZE) as u32;
    if queue_max < VIRTIO_QUEUE_NUM as u32 {
        warn!("virtio-disk-server: queue too small max={}", queue_max);
        return false;
    }
    if VIRTIO_QUEUE_NUM < XV6_DISK_MAX_IN_FLIGHT * DESCS_PER_REQUEST as usize {
        warn!("virtio-disk-server: queue too small for request slots");
        return false;
    }

    unsafe {
        ptr::write_bytes(XV6_VIRTIO_DMA_VADDR as *mut u8, 0, 4096);
    }
    USED_IDX.store(0, Ordering::Relaxed);

    common_write16(VIRTIO_PCI_QUEUE_SIZE, VIRTIO_QUEUE_NUM as u16);
    common_write64(VIRTIO_PCI_QUEUE_DESC, dma_pa(DESC_OFF));
    common_write64(VIRTIO_PCI_QUEUE_DRIVER, dma_pa(AVAIL_OFF));
    common_write64(VIRTIO_PCI_QUEUE_DEVICE, dma_pa(USED_OFF));
    QUEUE_NOTIFY_OFF.store(
        common_read16(VIRTIO_PCI_QUEUE_NOTIFY_OFF),
        Ordering::Relaxed,
    );
    common_write16(VIRTIO_PCI_QUEUE_ENABLE, 1);

    status |= VIRTIO_CONFIG_S_DRIVER_OK;
    write_device_status(status);
    info!(
        "virtio-disk-server: virtio-pci queue ready dev={}.{} dma={:#x}",
        device.function.device,
        device.function.function,
        DMA_PADDR.load(Ordering::Relaxed)
    );
    true
}

fn discover_virtio_blk() -> Option<VirtioPciDevice> {
    let mapped_devices = (XV6_PCIE_ECAM_MAP_SIZE / PCI_DEVICE_STRIDE).min(PCI_MAX_DEVICES);
    let mut device = 0u64;
    while device < mapped_devices {
        let mut function = 0u64;
        while function < PCI_FUNCTIONS_PER_DEVICE {
            let pci_function = PciFunction::new(device as u8, function as u8);
            let vendor = pci_function.read16(PCI_VENDOR_ID);
            if vendor == PCI_VENDOR_INVALID {
                break;
            }

            let device_id = pci_function.read16(PCI_DEVICE_ID);
            if vendor == PCI_VENDOR_VIRTIO && device_id == PCI_DEVICE_VIRTIO_BLK_MODERN {
                return discover_virtio_caps(pci_function);
            }

            let header_type = pci_function.read8(PCI_HEADER_TYPE);
            if function == 0 && (header_type & 0x80) == 0 {
                break;
            }
            function += 1;
        }
        device += 1;
    }
    None
}

fn discover_virtio_caps(function: PciFunction) -> Option<VirtioPciDevice> {
    if (function.read16(PCI_STATUS) & PCI_STATUS_CAP_LIST) == 0 {
        warn!("virtio-disk-server: virtio-pci device has no capability list");
        return None;
    }

    let mut common_cfg = 0;
    let mut notify_cfg = 0;
    let mut notify_off_multiplier = 0;
    let mut isr_cfg = 0;
    let mut cap = function.read8(PCI_CAP_PTR) & !0x3;
    let mut cap_count = 0usize;
    while cap != 0 && cap_count < PCI_MAX_CAPS {
        let cap_id = function.read8(cap as u64);
        let next = function.read8(cap as u64 + 1) & !0x3;
        if cap_id == PCI_CAP_ID_VNDR {
            let cap_len = function.read8(cap as u64 + 2);
            let cfg_type = function.read8(cap as u64 + 3);
            let bar = function.read8(cap as u64 + 4);
            let offset = function.read32(cap as u64 + 8);
            let length = function.read32(cap as u64 + 12);

            match cfg_type {
                VIRTIO_PCI_CAP_COMMON_CFG if cap_len >= VIRTIO_PCI_CAP_BASE_LEN => {
                    if common_cfg == 0 {
                        common_cfg = map_cap_region(function, bar, offset, length)?;
                    }
                }
                VIRTIO_PCI_CAP_NOTIFY_CFG if cap_len >= VIRTIO_PCI_NOTIFY_CAP_LEN => {
                    if notify_cfg == 0 {
                        notify_cfg = map_cap_region(function, bar, offset, length)?;
                        notify_off_multiplier = function.read32(cap as u64 + 16);
                    }
                }
                VIRTIO_PCI_CAP_ISR_CFG if cap_len >= VIRTIO_PCI_CAP_BASE_LEN => {
                    if isr_cfg == 0 {
                        isr_cfg = map_cap_region(function, bar, offset, length)?;
                    }
                }
                VIRTIO_PCI_CAP_DEVICE_CFG | VIRTIO_PCI_CAP_PCI_CFG => {}
                _ => {}
            }
        }
        cap = next;
        cap_count += 1;
    }

    if common_cfg == 0 || notify_cfg == 0 || isr_cfg == 0 {
        warn!(
            "virtio-disk-server: incomplete virtio-pci caps common={:#x} notify={:#x} isr={:#x}",
            common_cfg, notify_cfg, isr_cfg
        );
        return None;
    }
    info!(
        "virtio-disk-server: virtio-pci block device at dev={}.{} common={:#x} notify={:#x} isr={:#x}",
        function.device, function.function, common_cfg, notify_cfg, isr_cfg
    );
    Some(VirtioPciDevice {
        function,
        common_cfg,
        notify_cfg,
        notify_off_multiplier,
        isr_cfg,
    })
}

fn map_cap_region(function: PciFunction, bar: u8, offset: u32, length: u32) -> Option<u64> {
    if length == 0 {
        return None;
    }
    let bar_region = read_bar_region(function, bar)?;
    bar_region.translate(offset as u64, length as u64)
}

fn read_bar_region(function: PciFunction, bar: u8) -> Option<PciBarRegion> {
    if bar >= 6 {
        warn!("virtio-disk-server: invalid PCI BAR {}", bar);
        return None;
    }
    let reg = PCI_BAR0 + bar as u64 * 4;
    let raw = function.read32(reg);
    if (raw & PCI_BAR_IO) != 0 {
        let port = (raw & !0x3) as u64;
        return Some(PciBarRegion {
            kind: PciBarKind::Io,
            base: port,
        });
    }

    let mut paddr = (raw & !0xf) as u64;
    if (raw & PCI_BAR_MEM_TYPE_MASK) == PCI_BAR_MEM_TYPE_64 {
        if bar == 5 {
            warn!("virtio-disk-server: invalid 64-bit PCI BAR {}", bar);
            return None;
        }
        paddr |= (function.read32(reg + 4) as u64) << 32;
    }
    Some(PciBarRegion {
        kind: PciBarKind::Memory,
        base: paddr,
    })
}

fn enable_pci_function(function: PciFunction) {
    let mut command = function.read16(PCI_COMMAND);
    command |= PCI_COMMAND_IO_SPACE | PCI_COMMAND_MEMORY_SPACE | PCI_COMMAND_BUS_MASTER;
    command &= !PCI_COMMAND_INTX_DISABLE;
    function.write16(PCI_COMMAND, command);
}

fn read_device_features() -> u64 {
    common_write32(VIRTIO_PCI_DEVICE_FEATURE_SELECT, 0);
    let low = common_read32(VIRTIO_PCI_DEVICE_FEATURE);
    common_write32(VIRTIO_PCI_DEVICE_FEATURE_SELECT, 1);
    let high = common_read32(VIRTIO_PCI_DEVICE_FEATURE);
    ((high as u64) << 32) | low as u64
}

fn write_driver_features(features: u64) {
    common_write32(VIRTIO_PCI_DRIVER_FEATURE_SELECT, 0);
    common_write32(VIRTIO_PCI_DRIVER_FEATURE, features as u32);
    common_write32(VIRTIO_PCI_DRIVER_FEATURE_SELECT, 1);
    common_write32(VIRTIO_PCI_DRIVER_FEATURE, (features >> 32) as u32);
}

fn read_device_status() -> u32 {
    common_read8(VIRTIO_PCI_DEVICE_STATUS) as u32
}

fn write_device_status(status: u32) {
    common_write8(VIRTIO_PCI_DEVICE_STATUS, status as u8);
}

fn pci_notify_queue() {
    let notify = NOTIFY_CFG_VADDR.load(Ordering::Relaxed);
    let offset = QUEUE_NOTIFY_OFF.load(Ordering::Relaxed) as u64
        * NOTIFY_OFF_MULTIPLIER.load(Ordering::Relaxed) as u64;
    unsafe {
        ptr::write_volatile((notify + offset) as *mut u16, 0);
    }
}

fn feature_bit(bit: u32) -> u64 {
    1u64 << bit
}

fn write_desc(index: u16, addr: u64, len: u32, flags: u16, next: u16) {
    let off = DESC_OFF + index as u64 * 16;
    write64(off, addr);
    write32(off + 8, len);
    write16(off + 12, flags);
    write16(off + 14, next);
}

fn read16(offset: u64) -> u16 {
    unsafe { ptr::read_volatile(dma_va(offset) as *const u16) }
}

fn read32(offset: u64) -> u32 {
    unsafe { ptr::read_volatile(dma_va(offset) as *const u32) }
}

fn write16(offset: u64, value: u16) {
    unsafe { ptr::write_volatile(dma_va(offset) as *mut u16, value) }
}

fn write32(offset: u64, value: u32) {
    unsafe { ptr::write_volatile(dma_va(offset) as *mut u32, value) }
}

fn write64(offset: u64, value: u64) {
    unsafe { ptr::write_volatile(dma_va(offset) as *mut u64, value) }
}

fn common_read8(offset: u64) -> u8 {
    unsafe { ptr::read_volatile((COMMON_CFG_VADDR.load(Ordering::Relaxed) + offset) as *const u8) }
}

fn common_read16(offset: u64) -> u16 {
    unsafe { ptr::read_volatile((COMMON_CFG_VADDR.load(Ordering::Relaxed) + offset) as *const u16) }
}

fn common_read32(offset: u64) -> u32 {
    unsafe { ptr::read_volatile((COMMON_CFG_VADDR.load(Ordering::Relaxed) + offset) as *const u32) }
}

fn common_write8(offset: u64, value: u8) {
    unsafe {
        ptr::write_volatile(
            (COMMON_CFG_VADDR.load(Ordering::Relaxed) + offset) as *mut u8,
            value,
        )
    }
}

fn common_write16(offset: u64, value: u16) {
    unsafe {
        ptr::write_volatile(
            (COMMON_CFG_VADDR.load(Ordering::Relaxed) + offset) as *mut u16,
            value,
        )
    }
}

fn common_write32(offset: u64, value: u32) {
    unsafe {
        ptr::write_volatile(
            (COMMON_CFG_VADDR.load(Ordering::Relaxed) + offset) as *mut u32,
            value,
        )
    }
}

fn common_write64(offset: u64, value: u64) {
    unsafe {
        ptr::write_volatile(
            (COMMON_CFG_VADDR.load(Ordering::Relaxed) + offset) as *mut u64,
            value,
        )
    }
}

fn dma_pa(offset: u64) -> u64 {
    DMA_PADDR.load(Ordering::Relaxed) + offset
}

#[derive(Copy, Clone)]
struct VirtioPciDevice {
    function: PciFunction,
    common_cfg: u64,
    notify_cfg: u64,
    notify_off_multiplier: u32,
    isr_cfg: u64,
}

#[derive(Copy, Clone)]
struct PciFunction {
    device: u8,
    function: u8,
    vaddr: u64,
}

impl PciFunction {
    fn new(device: u8, function: u8) -> Self {
        Self {
            device,
            function,
            vaddr: XV6_PCIE_ECAM_VADDR
                + device as u64 * PCI_DEVICE_STRIDE
                + function as u64 * PCI_FUNCTION_STRIDE,
        }
    }

    fn read8(self, offset: u64) -> u8 {
        unsafe { ptr::read_volatile((self.vaddr + offset) as *const u8) }
    }

    fn read16(self, offset: u64) -> u16 {
        unsafe { ptr::read_volatile((self.vaddr + offset) as *const u16) }
    }

    fn read32(self, offset: u64) -> u32 {
        unsafe { ptr::read_volatile((self.vaddr + offset) as *const u32) }
    }

    fn write16(self, offset: u64, value: u16) {
        unsafe { ptr::write_volatile((self.vaddr + offset) as *mut u16, value) }
    }
}

#[derive(Copy, Clone)]
struct PciBarRegion {
    kind: PciBarKind,
    base: u64,
}

impl PciBarRegion {
    fn translate(self, offset: u64, length: u64) -> Option<u64> {
        let addr = self.base.checked_add(offset)?;
        match self.kind {
            PciBarKind::Memory => map_mem_paddr(addr, length),
            PciBarKind::Io => map_io_port(addr, length),
        }
    }
}

#[derive(Copy, Clone)]
enum PciBarKind {
    Memory,
    Io,
}

fn map_mem_paddr(paddr: u64, length: u64) -> Option<u64> {
    let offset = paddr.checked_sub(LOONGARCH64_PCIE_MEM_BASE)?;
    if offset.checked_add(length)? > XV6_PCIE_MEM_MAP_SIZE {
        warn!(
            "virtio-disk-server: PCI mem BAR range outside mapped window paddr={:#x} len={:#x}",
            paddr, length
        );
        return None;
    }
    Some(XV6_PCIE_MEM_VADDR + offset)
}

fn map_io_port(port: u64, length: u64) -> Option<u64> {
    let offset = port.checked_sub(LOONGARCH64_PCIE_IO_PORT_BASE)?;
    if offset.checked_add(length)? > XV6_PCIE_IO_MAP_SIZE {
        warn!(
            "virtio-disk-server: PCI I/O BAR range outside mapped window port={:#x} len={:#x}",
            port, length
        );
        return None;
    }
    Some(XV6_PCIE_IO_VADDR + offset)
}
