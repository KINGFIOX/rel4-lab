//! High-level kernel boot path: set up the rootserver VSpace, initial CSpace,
//! TCB caps, `seL4_BootInfo`, and then `sret` into the root task.

use core::cell::UnsafeCell;
use core::ptr;

use log_crate::{info, warn};

use crate::abi::bootinfo::{BootInfo, RootCNodeCapSlot, SlotRegion, UntypedDesc};
use crate::abi::constants::{
    KERNEL_ELF_BASE, MAX_NUM_BOOTINFO_UNTYPED_CAPS, MAX_NUM_NODES, ROOT_CNODE_SIZE_BITS,
    SEL4_MAX_UNTYPED_BITS, SEL4_MIN_UNTYPED_BITS, SEL4_SLOT_BITS,
};
use crate::arch::current::api::{ROOTSERVER_SSTATUS, UserContext, UserRegister};
use crate::arch::current::kernel::BOOT_PROFILE;
use crate::arch::current::kernel::trap::{
    init_timer, install_trap_vector, restore_user_context_with_kernel_lock,
};
use crate::arch::current::machine::paging::{
    LEAF_PARENT_COVERAGE_BITS, PAGE_SHIFT, PAGE_SIZE, PageTable, Pte, ROOT_CHILD_COVERAGE_BITS,
    pt_index,
};
use crate::arch::current::object::vspace::{
    alloc_pt_page, kpptr_to_paddr, make_boot_root_pt, paddr_to_kpptr, satp_for, switch_satp,
    user_flags,
};
use crate::arch::current::plat::{DEVICE_UNTYPED_REGIONS, FREE_RAM_REGIONS};
use crate::kernel::bootmem;
use crate::object::cap::{Cap, FRAME_RIGHTS_READ_WRITE, FRAME_SIZE_4K};
use crate::object::cnode::{Cte, cnode_bytes, install_initial_cap, with_cnode_at};
use crate::object::tcb::{self, Tcb};
use crate::object::untyped::{FreeRange, UntypedChunks, make_untyped_cap};

/// Where we place the user IPC buffer in the user's virtual address space.
/// Picked above the rootserver image to avoid collisions with any segment
/// the ELF was linked to.
pub const USER_IPC_BUFFER_VA: usize = 0x7FFF_F000;

/// Where we place the BootInfo frame (one 4 KiB page).
pub const USER_BOOTINFO_VA: usize = 0x7FFF_E000;

/// User stack top — we give the rootserver a small static stack right below
/// BootInfo so it can call its `crt0`. (sel4runtime sets up its own stack,
/// but only after main runs; the very early `_sel4_start` uses whatever sp
/// we hand it.)
pub const USER_STACK_TOP: usize = 0x7FFE_F000;
pub const USER_STACK_PAGES: usize = 16; // 64 KiB
const ROOTSERVER_ASID: u16 = 1;
const MAX_BOOT_USER_PAGING_CAPS: usize = 256;

#[repr(C)]
pub struct BootArgs {
    pub user_pstart: usize,
    pub user_pend: usize,
    pub pv_offset: usize, // PA - VA
    pub user_ventry: usize,
    pub dtb_pa: usize,
    pub dtb_size: usize,
    pub hart_id: usize,
    pub core_id: usize,
}

/// Static storage for the rootserver thread's TCB. Keep this as transparent
/// storage rather than wrapping the TCB in a lock; the cap pointer must address
/// the TCB object itself, while `context_ptr()` returns the embedded
/// `UserContext` for the trap restore path.
#[repr(transparent)]
struct RootTcbCell(UnsafeCell<Tcb>);

unsafe impl Sync for RootTcbCell {}

impl RootTcbCell {
    const fn new() -> Self {
        Self(UnsafeCell::new(Tcb::zero()))
    }

    fn kva(&self) -> u64 {
        self.0.get() as u64
    }

    fn with_mut<R>(&self, op: impl FnOnce(&mut Tcb) -> R) -> R {
        let tcb = unsafe { &mut *self.0.get() };
        op(tcb)
    }

    fn context_ptr(&self) -> *mut UserContext {
        unsafe { &raw mut (*self.0.get()).context }
    }
}

#[unsafe(no_mangle)]
static ROOTSERVER_TCB: RootTcbCell = RootTcbCell::new();

#[derive(Copy, Clone)]
struct BootUserPageTableCap {
    pt: *mut PageTable,
    mapped_addr: usize,
    level: usize,
}

impl BootUserPageTableCap {
    const fn empty() -> Self {
        Self {
            pt: core::ptr::null_mut(),
            mapped_addr: 0,
            level: 0,
        }
    }
}

struct BootUserPaging {
    root: *mut PageTable,
    caps: [BootUserPageTableCap; MAX_BOOT_USER_PAGING_CAPS],
    cap_count: usize,
}

impl BootUserPaging {
    fn new(root: *mut PageTable) -> Self {
        Self {
            root,
            caps: [BootUserPageTableCap::empty(); MAX_BOOT_USER_PAGING_CAPS],
            cap_count: 0,
        }
    }

    fn map_4k(&mut self, vaddr: usize, paddr: usize, flags: u64) {
        assert!(vaddr & (PAGE_SIZE - 1) == 0, "user VA is not 4K-aligned");
        assert!(paddr & (PAGE_SIZE - 1) == 0, "user PA is not 4K-aligned");
        let l1 = self.ensure_table(self.root, vaddr, 2);
        let l0 = self.ensure_table(l1, vaddr, 1);
        let slot = unsafe { &mut (*l0).entries[pt_index(vaddr, 0)] };
        assert!(
            !slot.is_valid(),
            "duplicate boot user mapping at VA {:#x}",
            vaddr
        );
        *slot = Pte::leaf(paddr as u64, flags);
        crate::arch::current::machine::tlb_flush_vaddr(vaddr);
        crate::kernel::smp::remote_sfence_vma_all();
    }

    fn ensure_table(
        &mut self,
        parent: *mut PageTable,
        vaddr: usize,
        parent_level: usize,
    ) -> *mut PageTable {
        let slot = unsafe { &mut (*parent).entries[pt_index(vaddr, parent_level)] };
        if slot.is_valid() {
            assert!(
                !slot.is_leaf(),
                "boot user mapping collided with a leaf at level {}",
                parent_level
            );
            return paddr_to_kpptr(slot.next_pt_paddr() as usize) as *mut PageTable;
        }

        let child = alloc_pt_page();
        *slot = Pte::next(kpptr_to_paddr(child as usize) as u64);
        let child_level = parent_level - 1;
        self.record_cap(
            child,
            align_down(vaddr, table_coverage_bits(child_level)),
            child_level,
        );
        child
    }

    fn record_cap(&mut self, pt: *mut PageTable, mapped_addr: usize, level: usize) {
        for i in 0..self.cap_count {
            if self.caps[i].pt == pt {
                return;
            }
        }
        assert!(
            self.cap_count < self.caps.len(),
            "too many boot user PageTable caps"
        );
        self.caps[self.cap_count] = BootUserPageTableCap {
            pt,
            mapped_addr,
            level,
        };
        self.cap_count += 1;
    }
}

const fn table_coverage_bits(level: usize) -> usize {
    match level {
        1 => ROOT_CHILD_COVERAGE_BITS,
        0 => LEAF_PARENT_COVERAGE_BITS,
        _ => PAGE_SHIFT,
    }
}

fn align_down(value: usize, bits: usize) -> usize {
    value & !((1usize << bits) - 1)
}

const _: () = {
    assert!(core::mem::size_of::<RootTcbCell>() == core::mem::size_of::<Tcb>());
    assert!(core::mem::align_of::<RootTcbCell>() == core::mem::align_of::<Tcb>());
};

/// Translate a kernel VA (either the kernel-ELF window or the PSpace
/// window) back to its physical address. Caps minted from RAM untypeds
/// use PSpace VAs; kernel-internal allocations (root CNode, IPC buffer,
/// stack) live in the boot pool inside the kernel ELF window.
#[inline]
fn kva_to_pa(kva: u64) -> u64 {
    use crate::abi::constants::{PADDR_BASE, PHYS_BASE_RAW, PPTR_BASE};
    if kva >= (KERNEL_ELF_BASE as u64) {
        kva - (KERNEL_ELF_BASE as u64) + (PHYS_BASE_RAW as u64)
    } else {
        kva - (PPTR_BASE as u64) + (PADDR_BASE as u64)
    }
}

/// Translate a physical address into the PSpace-window VA used as the
/// capability pointer for *device* untyped/frame caps. We don't actually
/// map PSpace in the page table — the kernel never dereferences device
/// memory directly — but we use the VA encoding so caps look identical
/// to what the C kernel would emit.
#[inline]
fn pa_to_pspace_va(pa: u64) -> u64 {
    use crate::abi::constants::{PADDR_BASE, PPTR_BASE};
    pa + (PPTR_BASE as u64) - (PADDR_BASE as u64)
}

/// Bootstrap the user environment and drop into U-mode.
pub fn bringup_rootserver(args: &BootArgs) -> ! {
    crate::kernel::smp::init_current_hart(args.hart_id, args.core_id);
    crate::arch::current::machine::fpu::init_current_core();
    install_trap_vector();
    init_timer();

    // --- VSpace -----------------------------------------------------------
    let root_pt = make_boot_root_pt();
    let satp = satp_for(root_pt, ROOTSERVER_ASID as u64);
    crate::kernel::smp::publish_kernel_satp(satp);
    unsafe { switch_satp(satp) };
    crate::machine::console::init();
    crate::arch::current::machine::irq::init();

    info!("microkernel: Rust kernel booted ({})", BOOT_PROFILE);
    info!(
        "  hart_id={} core_id={} dtb=0x{:x} ({} bytes)",
        args.hart_id, args.core_id, args.dtb_pa, args.dtb_size
    );
    info!("microkernel: bringing up rootserver");
    info!(
        "  user image: PA [{:#x}, {:#x}) VA offset={:#x} entry={:#x}",
        args.user_pstart, args.user_pend, args.pv_offset, args.user_ventry,
    );
    info!(
        "  root PT at VA {:#x} PA {:#x}",
        root_pt as usize,
        kpptr_to_paddr(root_pt as usize),
    );
    info!("  satp <- {:#x}", satp);

    // Map the rootserver image: PA = VA + pv_offset (elfloader convention).
    let mut boot_user_paging = BootUserPaging::new(root_pt);
    let user_va_start = args.user_pstart.wrapping_sub(args.pv_offset);
    let user_va_end = args.user_pend.wrapping_sub(args.pv_offset);
    map_range_4k_identity_from_elfloader(
        &mut boot_user_paging,
        user_va_start,
        user_va_end,
        args.pv_offset,
        user_flags(true, true, true),
    );

    // Allocate + map BootInfo, IPC buffer, user stack.
    let bi_kva = bootmem::alloc_page();
    let bi_pa = kpptr_to_paddr(bi_kva);
    boot_user_paging.map_4k(USER_BOOTINFO_VA, bi_pa, user_flags(true, true, false));

    let ipc_kva = bootmem::alloc_page();
    let ipc_pa = kpptr_to_paddr(ipc_kva);
    boot_user_paging.map_4k(USER_IPC_BUFFER_VA, ipc_pa, user_flags(true, true, false));

    for i in 0..USER_STACK_PAGES {
        let kva = bootmem::alloc_page();
        let pa = kpptr_to_paddr(kva);
        let va = USER_STACK_TOP - (i + 1) * PAGE_SIZE;
        boot_user_paging.map_4k(va, pa, user_flags(true, true, false));
    }

    let asid_pool_kva = bootmem::alloc_page();
    unsafe {
        let asid_pool = asid_pool_kva as *mut u64;
        *asid_pool.add(crate::object::asid::pool_index(ROOTSERVER_ASID)) = root_pt as u64;
    }

    // --- Root CNode -------------------------------------------------------
    //
    // Allocate the root CNode from the boot pool. sel4test uses the upstream
    // 13-bit root CNode, while the xv6 rootserver opts into a larger one for
    // service processes and usertest churn.
    let cnode_pages = cnode_bytes(ROOT_CNODE_SIZE_BITS) / PAGE_SIZE;
    let cnode_base = bootmem::alloc_pages(cnode_pages);
    let cnode_kva = cnode_base as u64;
    let cnode_slots = 1usize << ROOT_CNODE_SIZE_BITS;

    struct RootCnodeInit {
        next_slot: usize,
        user_image_paging_start: usize,
        user_image_paging_end: usize,
        untyped_start_slot: usize,
        untyped_end_slot: usize,
        device_start_slot: usize,
        device_end_slot: usize,
        user_image_frames_start: usize,
        user_image_frames_end: usize,
        init_ipc_buffer_tcb_cap: Cap,
        bi_untyped_count: usize,
        untyped_list_local: [UntypedDesc; MAX_NUM_BOOTINFO_UNTYPED_CAPS],
    }

    let init_root_cnode = |cnode: &mut [Cte]| -> RootCnodeInit {
        // Install the fixed initial caps that libsel4 expects at known slots.
        // Platform-specific caps that do not exist on this RISC-V profile are
        // left null, matching the root CNode slot numbering.
        install_initial_cap(
            cnode,
            RootCNodeCapSlot::InitThreadTcb.index(),
            Cap::new_thread(ROOTSERVER_TCB.kva()),
        );
        install_initial_cap(
            cnode,
            RootCNodeCapSlot::InitThreadCNode.index(),
            Cap::new_cnode(
                cnode_kva,
                ROOT_CNODE_SIZE_BITS as u64,
                0,
                64 - ROOT_CNODE_SIZE_BITS as u64,
            ),
        );
        let mut init_vspace_cap = Cap::new_page_table(root_pt as u64);
        init_vspace_cap.set_page_table_mapping(ROOTSERVER_ASID, 0);
        crate::object::asid::init_root(root_pt as u64, asid_pool_kva as u64);
        install_initial_cap(
            cnode,
            RootCNodeCapSlot::InitThreadVSpace.index(),
            init_vspace_cap,
        );
        install_initial_cap(
            cnode,
            RootCNodeCapSlot::IrqControl.index(),
            Cap::new_irq_control(),
        );
        install_initial_cap(cnode, RootCNodeCapSlot::Domain.index(), Cap::new_domain());
        install_initial_cap(
            cnode,
            RootCNodeCapSlot::AsidControl.index(),
            Cap::new_asid_control(),
        );
        install_initial_cap(
            cnode,
            RootCNodeCapSlot::InitThreadAsidPool.index(),
            Cap::new_asid_pool(0, asid_pool_kva as u64),
        );

        let mut bootinfo_frame_cap = Cap::new_frame(
            pa_to_pspace_va(bi_pa as u64),
            FRAME_SIZE_4K,
            FRAME_RIGHTS_READ_WRITE,
            false,
        );
        bootinfo_frame_cap.set_frame_mapped_addr(USER_BOOTINFO_VA as u64);
        bootinfo_frame_cap.set_frame_mapped_asid(ROOTSERVER_ASID);
        install_initial_cap(
            cnode,
            RootCNodeCapSlot::BootInfoFrame.index(),
            bootinfo_frame_cap,
        );

        let mut init_ipc_buffer_cap = Cap::new_frame(
            pa_to_pspace_va(ipc_pa as u64),
            FRAME_SIZE_4K,
            FRAME_RIGHTS_READ_WRITE,
            false,
        );
        init_ipc_buffer_cap.set_frame_mapped_addr(USER_IPC_BUFFER_VA as u64);
        init_ipc_buffer_cap.set_frame_mapped_asid(ROOTSERVER_ASID);
        install_initial_cap(
            cnode,
            RootCNodeCapSlot::InitThreadIpcBuffer.index(),
            init_ipc_buffer_cap,
        );

        let mut init_ipc_buffer_tcb_cap = init_ipc_buffer_cap;
        init_ipc_buffer_tcb_cap.set_frame_mapped_addr(0);
        init_ipc_buffer_tcb_cap.set_frame_mapped_asid(0);
        {
            let ctable_src = &mut cnode[RootCNodeCapSlot::InitThreadCNode.index()] as *mut Cte;
            let vtable_src = &mut cnode[RootCNodeCapSlot::InitThreadVSpace.index()] as *mut Cte;
            let buffer_src = &mut cnode[RootCNodeCapSlot::InitThreadIpcBuffer.index()] as *mut Cte;
            let cspace_guard = crate::object::cnode::lock_cspace();
            ROOTSERVER_TCB.with_mut(|rs| unsafe {
                let rs_ptr = rs as *mut Tcb;
                crate::object::cnode::cte_insert_locked(
                    &cspace_guard,
                    (*ctable_src).cap,
                    ctable_src,
                    tcb::cap_slot(rs_ptr, tcb::TCB_CTABLE_SLOT),
                );
                crate::object::cnode::cte_insert_locked(
                    &cspace_guard,
                    (*vtable_src).cap,
                    vtable_src,
                    tcb::cap_slot(rs_ptr, tcb::TCB_VTABLE_SLOT),
                );
                crate::object::cnode::cte_insert_locked(
                    &cspace_guard,
                    init_ipc_buffer_tcb_cap,
                    buffer_src,
                    tcb::cap_slot(rs_ptr, tcb::TCB_BUFFER_SLOT),
                );
            });
        }
        let mut next_slot = RootCNodeCapSlot::NumInitialCaps.index();

        let (user_image_paging_start, user_image_paging_end) =
            install_boot_user_paging_caps(cnode, &boot_user_paging, &mut next_slot);
        let untyped_start_slot = next_slot;
        let mut bi_untyped_count = 0usize;
        let mut untyped_list_local: [UntypedDesc; MAX_NUM_BOOTINFO_UNTYPED_CAPS] = [const {
            UntypedDesc {
                paddr: 0,
                size_bits: 0,
                is_device: 0,
                _padding: [0; 6],
            }
        };
            MAX_NUM_BOOTINFO_UNTYPED_CAPS];

        // --- Free memory enumeration -> untyped caps ------------------------
        for &(start_pa, end_pa) in FREE_RAM_REGIONS {
            let free_range = FreeRange {
                start_kva: pa_to_pspace_va(start_pa),
                size: end_pa - start_pa,
            };
            for (base_kva, bits) in UntypedChunks::new(free_range) {
                if next_slot >= cnode.len() {
                    warn!("  warn: root CNode full while enumerating untypeds");
                    break;
                }
                if bi_untyped_count >= MAX_NUM_BOOTINFO_UNTYPED_CAPS {
                    break;
                }
                let cap = make_untyped_cap(base_kva, bits, false);
                install_initial_cap(cnode, next_slot, cap);
                untyped_list_local[bi_untyped_count] = UntypedDesc {
                    paddr: kva_to_pa(base_kva),
                    size_bits: bits,
                    is_device: 0,
                    _padding: [0; 6],
                };
                next_slot += 1;
                bi_untyped_count += 1;
            }
        }

        // --- Device untypeds (QEMU virt MMIO) --------------------------------
        let device_start_slot = next_slot;
        for &(start_pa, end_pa) in DEVICE_UNTYPED_REGIONS {
            let device_range = FreeRange {
                start_kva: pa_to_pspace_va(start_pa),
                size: end_pa - start_pa,
            };
            for (base_kva, bits) in UntypedChunks::new(device_range) {
                if next_slot >= cnode.len() || bi_untyped_count >= MAX_NUM_BOOTINFO_UNTYPED_CAPS {
                    break;
                }
                let cap = make_untyped_cap(base_kva, bits, true);
                install_initial_cap(cnode, next_slot, cap);
                untyped_list_local[bi_untyped_count] = UntypedDesc {
                    paddr: kva_to_pa(base_kva),
                    size_bits: bits,
                    is_device: 1,
                    _padding: [0; 6],
                };
                next_slot += 1;
                bi_untyped_count += 1;
            }
        }
        let device_end_slot = next_slot;
        let untyped_end_slot = next_slot;

        // --- User image frames -----------------------------------------------
        //
        // The rootserver's vspace library (`sel4utils`) needs to know which
        // VA range is occupied by its own statically-mapped ELF image. With
        // no `userImageFrames` entries in BootInfo, the library treats the
        // image's VAs as free and happily Page_Map's new frames on top of
        // them — silently overwriting the .text/.data PTEs and crashing the
        // moment the rootserver next dereferences something from there.
        //
        // Install one 4 KiB Frame cap per image page; the user-VA is recorded
        // in the cap's `mapped_address` field so the vspace library's "where
        // is this page?" query has a real answer. Memory itself is already
        // mapped at boot time, so we don't add new PTEs here.
        let user_image_frames_start = next_slot;
        let user_va_start_aligned =
            args.user_pstart.wrapping_sub(args.pv_offset) & !(PAGE_SIZE - 1);
        let user_va_end_aligned =
            (args.user_pend.wrapping_sub(args.pv_offset) + PAGE_SIZE - 1) & !(PAGE_SIZE - 1);
        let mut va = user_va_start_aligned;
        while va < user_va_end_aligned {
            if next_slot >= cnode.len() {
                warn!("  warn: root CNode full while installing user-image frame caps");
                break;
            }
            let pa = va.wrapping_add(args.pv_offset) as u64;
            let frame_kva = pa_to_pspace_va(pa);
            let mut cap = Cap::new_frame(frame_kva, FRAME_SIZE_4K, FRAME_RIGHTS_READ_WRITE, false);
            cap.set_frame_mapped_addr(va as u64);
            cap.set_frame_mapped_asid(ROOTSERVER_ASID);
            install_initial_cap(cnode, next_slot, cap);
            next_slot += 1;
            va += PAGE_SIZE;
        }
        let user_image_frames_end = next_slot;

        RootCnodeInit {
            next_slot,
            user_image_paging_start,
            user_image_paging_end,
            untyped_start_slot,
            untyped_end_slot,
            device_start_slot,
            device_end_slot,
            user_image_frames_start,
            user_image_frames_end,
            init_ipc_buffer_tcb_cap,
            bi_untyped_count,
            untyped_list_local,
        }
    };

    let RootCnodeInit {
        next_slot,
        user_image_paging_start,
        user_image_paging_end,
        untyped_start_slot,
        untyped_end_slot,
        device_start_slot,
        device_end_slot,
        user_image_frames_start,
        user_image_frames_end,
        init_ipc_buffer_tcb_cap,
        bi_untyped_count,
        untyped_list_local,
    } = unsafe { with_cnode_at(cnode_base as *mut u8, ROOT_CNODE_SIZE_BITS, init_root_cnode) };

    info!(
        "  user image paging: slots {}..{} ({} caps)",
        user_image_paging_start,
        user_image_paging_end,
        user_image_paging_end - user_image_paging_start,
    );
    info!(
        "  user image frames: slots {}..{} ({} caps)",
        user_image_frames_start,
        user_image_frames_end,
        user_image_frames_end - user_image_frames_start,
    );
    info!(
        "  device untyped: slots {}..{} ({} caps)",
        device_start_slot,
        device_end_slot,
        device_end_slot - device_start_slot,
    );
    info!(
        "  root CNode: {} initial caps, {} untyped (slots {}..{}), {} slots free",
        RootCNodeCapSlot::NumInitialCaps.raw(),
        bi_untyped_count,
        untyped_start_slot,
        untyped_end_slot,
        cnode_slots - next_slot,
    );

    // --- Register rootserver thread state for syscall path ---------------
    let cnode_cap_for_thread = Cap::new_cnode(
        cnode_kva,
        ROOT_CNODE_SIZE_BITS as u64,
        0,
        64 - ROOT_CNODE_SIZE_BITS as u64,
    );
    crate::api::thread::install_rootserver(
        cnode_base as *mut crate::object::cnode::Cte,
        ROOT_CNODE_SIZE_BITS as u32,
        cnode_cap_for_thread,
        init_ipc_buffer_tcb_cap.frame_base_ptr() as *mut u64,
        USER_IPC_BUFFER_VA as u64,
        root_pt as u64,
    );
    ROOTSERVER_TCB.with_mut(|rs| {
        rs.ipc_buffer_uva = USER_IPC_BUFFER_VA as u64;
        rs.ipc_buffer_kva = init_ipc_buffer_tcb_cap.frame_base_ptr();
    });

    // --- Populate BootInfo -----------------------------------------------
    let bi = bi_kva as *mut BootInfo;
    unsafe {
        ptr::write_bytes(bi as *mut u8, 0, core::mem::size_of::<BootInfo>());
        (*bi).node_id = 0;
        (*bi).num_nodes = MAX_NUM_NODES as u64;
        (*bi).num_io_pt_levels = 0;
        (*bi).ipc_buffer = USER_IPC_BUFFER_VA as u64;
        (*bi).empty = SlotRegion {
            start: next_slot as u64,
            end: cnode_slots as u64,
        };
        (*bi).user_image_frames = SlotRegion {
            start: user_image_frames_start as u64,
            end: user_image_frames_end as u64,
        };
        (*bi).user_image_paging = SlotRegion {
            start: user_image_paging_start as u64,
            end: user_image_paging_end as u64,
        };
        (*bi).io_space_caps = SlotRegion { start: 0, end: 0 };
        (*bi).extra_bi_pages = SlotRegion { start: 0, end: 0 };
        (*bi).init_thread_cnode_size_bits = ROOT_CNODE_SIZE_BITS as u64;
        (*bi).init_thread_domain = 0;
        (*bi).untyped = SlotRegion {
            start: untyped_start_slot as u64,
            end: untyped_end_slot as u64,
        };
        (*bi).untyped_list = untyped_list_local;
        let _ = (SEL4_MIN_UNTYPED_BITS, SEL4_MAX_UNTYPED_BITS, SEL4_SLOT_BITS);
    }

    info!(
        "  bootinfo: ipc@{:#x} cnode_bits={} untyped=[{}..{}) ({} caps)",
        USER_IPC_BUFFER_VA,
        ROOT_CNODE_SIZE_BITS,
        untyped_start_slot,
        untyped_end_slot,
        bi_untyped_count,
    );

    // --- Switch to user mode ---------------------------------------------
    let t = ROOTSERVER_TCB.with_mut(|t| {
        // sstatus: SPIE=1 (sret re-enables interrupts),
        //          SUM=1  (kernel can touch user memory),
        //          SPP=0  (sret enters U-mode).
        t.context.pc = args.user_ventry as u64;
        t.context.restart_pc = args.user_ventry as u64;
        t.context.sstatus = ROOTSERVER_SSTATUS;
        t.context.regs[UserRegister::A0.index()] = USER_BOOTINFO_VA as u64;
        t.context.regs[UserRegister::A1.index()] = 0;
        t.context.regs[UserRegister::Sp.index()] = USER_STACK_TOP as u64;
        t.affinity = crate::kernel::smp::current_core_id() as u8;
        t.state = crate::object::tcb::ThreadState::Running as u8;
        t as *mut Tcb
    });
    tcb::set_current(t);
    crate::arch::current::machine::fpu::lazy_restore(t);
    // Seed the scheduler's runqueue with the rootserver, so
    // `schedule()` always has a runnable TCB to return.
    unsafe {
        tcb::enqueue(t);
    }
    log_arch_restore_state(root_pt, args.user_ventry, USER_BOOTINFO_VA, USER_STACK_TOP);
    info!("  entering user mode at {:#x}", args.user_ventry);
    info!("  --- transferring control to rootserver ---");
    let kernel_lock = crate::kernel::smp::KernelLockGuard::lock();
    crate::kernel::smp::release_secondary_harts();
    unsafe {
        restore_user_context_with_kernel_lock(ROOTSERVER_TCB.context_ptr(), kernel_lock);
    }
}

#[cfg(target_arch = "loongarch64")]
fn log_arch_restore_state(
    root_pt: *mut PageTable,
    entry: usize,
    bootinfo: usize,
    stack_top: usize,
) {
    use crate::abi::constants::WORD_BYTES;
    use crate::arch::current::machine::csr;

    info!(
        "  loongarch64 restore csr: crmd={:#x} prmd={:#x} eentry={:#x} pgdl={:#x} pgdh={:#x} asid={:#x} pwcl={:#x} pwch={:#x} stlbps={:#x}",
        csr::crmd(),
        csr::prmd(),
        csr::eentry(),
        csr::pgdl(),
        csr::pgdh(),
        csr::asid(),
        csr::pwcl(),
        csr::pwch(),
        csr::stlbps(),
    );
    for (label, va) in [
        ("entry", entry),
        ("bootinfo", bootinfo),
        ("stack", stack_top - WORD_BYTES),
    ] {
        log_loongarch64_pte_walk(root_pt, label, va);
    }
}

#[cfg(not(target_arch = "loongarch64"))]
fn log_arch_restore_state(
    _root_pt: *mut PageTable,
    _entry: usize,
    _bootinfo: usize,
    _stack_top: usize,
) {
}

#[cfg(target_arch = "loongarch64")]
fn log_loongarch64_pte_walk(root_pt: *mut PageTable, label: &str, va: usize) {
    let l2_idx = pt_index(va, 2);
    let l1_idx = pt_index(va, 1);
    let l0_idx = pt_index(va, 0);
    let l2 = unsafe { (*root_pt).entries[l2_idx] };
    if !l2.is_valid() || l2.is_leaf() {
        info!(
            "  loongarch64 pte {} va={:#x}: l2[{}]={:#x}",
            label,
            va,
            l2_idx,
            l2.raw(),
        );
        return;
    }
    let l1_pt = paddr_to_kpptr(l2.next_pt_paddr() as usize) as *mut PageTable;
    let l1 = unsafe { (*l1_pt).entries[l1_idx] };
    if !l1.is_valid() || l1.is_leaf() {
        info!(
            "  loongarch64 pte {} va={:#x}: l2[{}]={:#x} l1[{}]={:#x}",
            label,
            va,
            l2_idx,
            l2.raw(),
            l1_idx,
            l1.raw(),
        );
        return;
    }
    let l0_pt = paddr_to_kpptr(l1.next_pt_paddr() as usize) as *mut PageTable;
    let l0 = unsafe { (*l0_pt).entries[l0_idx] };
    info!(
        "  loongarch64 pte {} va={:#x}: l2[{}]={:#x} l1[{}]={:#x} l0[{}]={:#x} pa={:#x}",
        label,
        va,
        l2_idx,
        l2.raw(),
        l1_idx,
        l1.raw(),
        l0_idx,
        l0.raw(),
        l0.leaf_pa(),
    );
}

/// Map a contiguous VA range of the user image to its PA range. Both VAs
/// and PAs are required to be 4 KiB aligned; the caller passes the
/// elfloader's `pv_offset` to recover PA from VA (PA = VA + pv_offset).
fn map_range_4k_identity_from_elfloader(
    paging: &mut BootUserPaging,
    va_start: usize,
    va_end: usize,
    pv_offset: usize,
    flags: u64,
) {
    let start = va_start & !(PAGE_SIZE - 1);
    let end = (va_end + PAGE_SIZE - 1) & !(PAGE_SIZE - 1);
    let mut va = start;
    while va < end {
        let pa = va.wrapping_add(pv_offset);
        paging.map_4k(va, pa, flags);
        va += PAGE_SIZE;
    }
}

fn install_boot_user_paging_caps(
    cnode: &mut [Cte],
    paging: &BootUserPaging,
    next_slot: &mut usize,
) -> (usize, usize) {
    let start = *next_slot;
    let mut emitted = [false; MAX_BOOT_USER_PAGING_CAPS];

    for level in [1usize, 0usize] {
        loop {
            let mut best: Option<usize> = None;
            for i in 0..paging.cap_count {
                if emitted[i] || paging.caps[i].level != level {
                    continue;
                }
                if best
                    .map(|best_idx| paging.caps[i].mapped_addr < paging.caps[best_idx].mapped_addr)
                    .unwrap_or(true)
                {
                    best = Some(i);
                }
            }

            let Some(i) = best else {
                break;
            };
            assert!(
                *next_slot < cnode.len(),
                "root CNode full while installing boot user PageTable caps"
            );
            let mut cap = Cap::new_page_table(paging.caps[i].pt as u64);
            cap.set_page_table_mapping(ROOTSERVER_ASID, paging.caps[i].mapped_addr as u64);
            install_initial_cap(cnode, *next_slot, cap);
            *next_slot += 1;
            emitted[i] = true;
        }
    }

    (start, *next_slot)
}
