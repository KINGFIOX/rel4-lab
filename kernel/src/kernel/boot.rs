//! High-level kernel boot path: set up the rootserver VSpace, initial CSpace,
//! TCB caps, `seL4_BootInfo`, and then return to the root task.

use core::ptr;

use log_crate::{info, warn};

use crate::abi::bootinfo::{BootInfo, RootCNodeCapSlot, SlotRegion, UntypedDesc};
use crate::abi::constants::{
    KERNEL_ELF_BASE, MAX_NUM_BOOTINFO_UNTYPED_CAPS, MAX_NUM_NODES, PT_INDEX_BITS,
    ROOT_CNODE_SIZE_BITS, SEL4_MAX_UNTYPED_BITS, SEL4_MIN_UNTYPED_BITS, SEL4_SLOT_BITS,
};
use crate::arch::current::kernel::BOOT_PROFILE;
use crate::arch::current::kernel::trap::{
    init_timer, install_trap_vector, restore_user_context_with_kernel_lock,
};
use crate::arch::current::machine::paging::{
    PAGE_SHIFT, PAGE_SIZE, PageTable, Pte, ROOT_LEVEL, pt_index,
};
use crate::arch::current::object::vspace::{
    alloc_pt_page, kpptr_to_paddr, make_boot_root_pt, paddr_to_kpptr, switch_vspace, user_flags,
    vspace_root_for,
};
use crate::arch::current::plat::{DEVICE_UNTYPED_REGIONS, FREE_RAM_REGIONS};
use crate::arch::current::sel4_arch;
use crate::kernel::bootmem;
use crate::ktypes::addr::{Kva, Paddr, UserVa};
use crate::ktypes::objref::ObjCell;
use crate::object::cap::{Cap, FRAME_RIGHTS_READ_WRITE, FRAME_SIZE_4K};
use crate::object::cnode::{CNode, cnode_at, cnode_bytes, install_initial_cap};
use crate::object::tcb::{self, Tcb, TcbRef, ThreadState};
use crate::object::untyped::{FreeRange, UntypedChunks, make_untyped_cap};

/// Where we place the user IPC buffer in the user's virtual address space.
/// Picked above the rootserver image to avoid collisions with any segment
/// the ELF was linked to.
pub const USER_IPC_BUFFER_VA: usize = 0x7FFF_D000;

/// Where we place the BootInfo frame (one 4 KiB page). Extra bootinfo
/// starts at `USER_BOOTINFO_VA + PAGE_SIZE`, matching libsel4simple.
pub const USER_BOOTINFO_VA: usize = 0x7FFF_E000;
#[cfg(target_arch = "x86_64")]
pub const USER_EXTRA_BI_VA: usize = USER_BOOTINFO_VA + PAGE_SIZE;
#[cfg(target_arch = "x86_64")]
const SEL4_BOOTINFO_HEADER_X86_TSC_FREQ: u64 = 5;
#[cfg(target_arch = "x86_64")]
const EXTRA_BI_TSC_CHUNK_LEN: u64 = 2 * 8 + 4;
#[cfg(target_arch = "x86_64")]
const QEMU_TSC_FREQ_MHZ: u32 = 1000;

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
    pub cpu_id: usize,
    pub core_id: usize,
}

/// Static storage for the rootserver thread's TCB. The rootserver is the one
/// thread the kernel creates itself rather than having user-space retype it,
/// but it is reached the same way as any other: through a handle.
#[unsafe(no_mangle)]
static ROOTSERVER_TCB: ObjCell<Tcb> = ObjCell::new(Tcb::zero());

/// Handle for the rootserver TCB.
fn rootserver_tcb() -> TcbRef {
    ROOTSERVER_TCB.get()
}

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
        let mut table = self.root;
        let mut parent_level = ROOT_LEVEL;
        while parent_level > 0 {
            table = self.ensure_table(table, vaddr, parent_level);
            parent_level -= 1;
        }
        // SAFETY: `table` came from `ensure_table`, which returns either the
        // root this builder was constructed with or a page freshly allocated
        // from the boot pool, and nothing else refers to it.
        let slot = unsafe { &mut (*table).entries[pt_index(vaddr, 0)] };
        assert!(
            !slot.is_valid(),
            "duplicate boot user mapping at VA {:#x}",
            vaddr
        );
        *slot = Pte::leaf(paddr as u64, flags);
        crate::arch::current::machine::tlb_flush_vaddr(vaddr);
        crate::kernel::smp::remote_tlb_flush_all();
    }

    fn ensure_table(
        &mut self,
        parent: *mut PageTable,
        vaddr: usize,
        parent_level: usize,
    ) -> *mut PageTable {
        // SAFETY: as `map_4k` — `parent` is a live boot-pool page table and
        // this builder is the only thing writing it.
        let slot = unsafe { &mut (*parent).entries[pt_index(vaddr, parent_level)] };
        if slot.is_valid() {
            assert!(
                !slot.is_leaf(),
                "boot user mapping collided with a leaf at level {}",
                parent_level
            );
            return paddr_to_kpptr(Paddr::from_u64(slot.next_pt_paddr())).as_ptr();
        }

        let child = alloc_pt_page();
        *slot = Pte::next(kpptr_to_paddr(Kva::new(child as usize)).as_u64());
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
    PAGE_SHIFT + PT_INDEX_BITS * (level + 1)
}

fn align_down(value: usize, bits: usize) -> usize {
    value & !((1usize << bits) - 1)
}

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
    crate::kernel::smp::init_current_cpu(args.cpu_id, args.core_id);
    crate::arch::current::machine::fpu::init_current_core();
    install_trap_vector();
    init_timer();

    // --- VSpace -----------------------------------------------------------
    let root_pt = make_boot_root_pt();
    let vspace_root = vspace_root_for(root_pt, ROOTSERVER_ASID as u64);
    crate::kernel::smp::publish_kernel_vspace(vspace_root);
    // SAFETY: `vspace_root` was just composed from the boot root page table,
    // which maps the kernel image and the PSpace window the kernel runs from.
    unsafe { switch_vspace(vspace_root) };
    crate::machine::console::init();
    crate::arch::current::machine::irq::init();

    info!("microkernel: Rust kernel booted ({})", BOOT_PROFILE);
    info!(
        "  cpu_id={} core_id={} dtb=0x{:x} ({} bytes)",
        args.cpu_id, args.core_id, args.dtb_pa, args.dtb_size
    );
    info!("microkernel: bringing up rootserver");
    tcb::create_idle_threads();
    tcb::switch_to_idle_thread();
    info!(
        "  user image: PA [{:#x}, {:#x}) VA offset={:#x} entry={:#x}",
        args.user_pstart, args.user_pend, args.pv_offset, args.user_ventry,
    );
    info!(
        "  root PT at VA {:#x} PA {:#x}",
        root_pt as usize,
        kpptr_to_paddr(Kva::new(root_pt as usize)).raw(),
    );
    info!("  vspace root <- {:#x}", vspace_root);

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
    let bi_pa = kpptr_to_paddr(Kva::new(bi_kva)).raw();
    boot_user_paging.map_4k(USER_BOOTINFO_VA, bi_pa, user_flags(true, true, false));

    #[cfg(target_arch = "x86_64")]
    {
        let extra_bi_kva = bootmem::alloc_page();
        let extra_bi_pa = kpptr_to_paddr(Kva::new(extra_bi_kva)).raw();
        boot_user_paging.map_4k(USER_EXTRA_BI_VA, extra_bi_pa, user_flags(true, true, false));
        // SAFETY: the boot pool just handed out this page exclusively, and the
        // extra-bootinfo header fits well inside it.
        unsafe {
            ptr::write_bytes(extra_bi_kva as *mut u8, 0, PAGE_SIZE);
            let header = extra_bi_kva as *mut u64;
            *header = SEL4_BOOTINFO_HEADER_X86_TSC_FREQ;
            *header.add(1) = EXTRA_BI_TSC_CHUNK_LEN;
            *(extra_bi_kva as *mut u32).add(4) = QEMU_TSC_FREQ_MHZ;
        }
    }

    let ipc_kva = bootmem::alloc_page();
    let ipc_pa = kpptr_to_paddr(Kva::new(ipc_kva)).raw();
    boot_user_paging.map_4k(USER_IPC_BUFFER_VA, ipc_pa, user_flags(true, true, false));

    for i in 0..USER_STACK_PAGES {
        let kva = bootmem::alloc_page();
        let pa = kpptr_to_paddr(Kva::new(kva)).raw();
        let va = USER_STACK_TOP - (i + 1) * PAGE_SIZE;
        boot_user_paging.map_4k(va, pa, user_flags(true, true, false));
    }

    let asid_pool_kva = bootmem::alloc_page();
    // SAFETY: a freshly allocated boot page, used here as the rootserver's
    // ASID pool; the index is within one page of entries.
    unsafe {
        let asid_pool = asid_pool_kva as *mut u64;
        *asid_pool.add(crate::object::asid::pool_index(ROOTSERVER_ASID)) = root_pt as u64;
    }

    // --- Root CNode -------------------------------------------------------
    //
    // Allocate the root CNode from the boot pool. sel4test uses the upstream
    // 13-bit root CNode, while linux-compat opts into a larger one for
    // service processes and LTP process churn.
    let cnode_pages = cnode_bytes(ROOT_CNODE_SIZE_BITS) / PAGE_SIZE;
    let cnode_base = bootmem::alloc_pages(cnode_pages);
    let cnode_kva = cnode_base as u64;
    let cnode_slots = 1usize << ROOT_CNODE_SIZE_BITS;
    // SAFETY: the boot pool just handed out enough zeroed, page-aligned pages
    // to hold this many slots, and nothing else refers to that memory.
    let root_cnode =
        unsafe { cnode_at(cnode_kva, ROOT_CNODE_SIZE_BITS) }.expect("root CNode allocation failed");

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

    let init_root_cnode = |cnode: CNode| -> RootCnodeInit {
        // Install the fixed initial caps that libsel4 expects at known slots.
        // Platform-specific caps that do not exist on this RISC-V profile are
        // left null, matching the root CNode slot numbering.
        install_initial_cap(
            cnode,
            RootCNodeCapSlot::InitThreadTcb.index(),
            Cap::new_thread(rootserver_tcb().kva()),
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
        #[cfg(target_arch = "x86_64")]
        install_initial_cap(
            cnode,
            RootCNodeCapSlot::IoPortControl.index(),
            Cap::new_io_port_control(),
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
            let rootserver = rootserver_tcb();
            let slot = |index: usize| cnode.get(index).expect("initial cap slot in range");
            let tcb_slot =
                |index: usize| rootserver.cap_slot(index).expect("TCB cap slot in range");
            let ctable_src = slot(RootCNodeCapSlot::InitThreadCNode.index());
            let vtable_src = slot(RootCNodeCapSlot::InitThreadVSpace.index());
            let buffer_src = slot(RootCNodeCapSlot::InitThreadIpcBuffer.index());
            ctable_src.cte_insert(ctable_src.cap(), tcb_slot(tcb::TCB_CTABLE_SLOT));
            vtable_src.cte_insert(vtable_src.cap(), tcb_slot(tcb::TCB_VTABLE_SLOT));
            buffer_src.cte_insert(init_ipc_buffer_tcb_cap, tcb_slot(tcb::TCB_BUFFER_SLOT));
            crate::object::reply::setup_reply_master(rootserver);
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
    } = init_root_cnode(root_cnode);

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

    // --- Register rootserver thread state for the syscall path ------------
    //
    // Cap lookups and IPC-buffer access follow the current TCB's own slots, so
    // the only thing left to record is where its IPC buffer lives.
    rootserver_tcb().set_ipc_buffer(UserVa::new(USER_IPC_BUFFER_VA), init_ipc_buffer_tcb_cap);

    // --- Populate BootInfo -----------------------------------------------
    let bi = bi_kva as *mut BootInfo;
    // SAFETY: `bi_kva` is a boot page mapped for the rootserver, exclusively
    // owned here, and `BootInfo` fits in one page.
    unsafe {
        ptr::write_bytes(bi as *mut u8, 0, core::mem::size_of::<BootInfo>());
        #[cfg(target_arch = "x86_64")]
        {
            (*bi).extra_len = EXTRA_BI_TSC_CHUNK_LEN;
        }
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
    let rootserver = rootserver_tcb();
    rootserver.with_context_mut(|context| {
        sel4_arch::init_rootserver_context(
            context,
            args.user_ventry as u64,
            USER_STACK_TOP as u64,
            USER_BOOTINFO_VA as u64,
        );
    });
    rootserver.set_initial_affinity(crate::kernel::smp::current_core_id() as u8);
    rootserver.set_state(ThreadState::Running);
    tcb::set_current(Some(rootserver));
    crate::arch::current::machine::fpu::lazy_restore(rootserver);
    // Seed the scheduler's runqueue with the rootserver, so `schedule()`
    // always has a runnable TCB to return.
    rootserver.enqueue();
    log_arch_restore_state(root_pt, args.user_ventry, USER_BOOTINFO_VA, USER_STACK_TOP);
    info!("  entering user mode at {:#x}", args.user_ventry);
    info!("  --- transferring control to rootserver ---");
    crate::arch::current::kernel::start_application_processors();
    let kernel_lock = crate::kernel::smp::KernelLockGuard::lock();
    crate::kernel::smp::release_secondary_cpus();
    // SAFETY: the rootserver's context has been fully initialised above, its
    // VSpace is installed, and the kernel lock is handed over to the restore
    // path which releases it as it returns to user mode.
    unsafe {
        restore_user_context_with_kernel_lock(rootserver.context_ptr(), kernel_lock);
    }
}

fn log_arch_restore_state(
    _root_pt: *mut PageTable,
    _entry: usize,
    _bootinfo: usize,
    _stack_top: usize,
) {
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
    cnode: CNode,
    paging: &BootUserPaging,
    next_slot: &mut usize,
) -> (usize, usize) {
    let start = *next_slot;
    let mut emitted = [false; MAX_BOOT_USER_PAGING_CAPS];

    for level in (0..ROOT_LEVEL).rev() {
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
