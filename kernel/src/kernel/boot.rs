//! High-level kernel boot path (M2.2): set up the user image's VSpace,
//! construct a minimal `seL4_BootInfo`, and `sret` into the root task.
//!
//! We are intentionally not implementing CSpace / TCB / capability bookkeeping
//! yet — those come in M3. The goal of M2.2 is just to get sel4test-driver
//! into user mode far enough to print something via `seL4_DebugPutChar`.

use core::ptr;

use crate::abi::bootinfo::{self as bi_slot, BootInfo, SlotRegion, UntypedDesc};
use crate::abi::constants::{
    KERNEL_ELF_BASE, MAX_NUM_BOOTINFO_UNTYPED_CAPS, ROOT_CNODE_SIZE_BITS, SEL4_MAX_UNTYPED_BITS,
    SEL4_MIN_UNTYPED_BITS, SEL4_SLOT_BITS,
};
use crate::arch::riscv64::sv39::{PAGE_SIZE, PageTable};
use crate::arch::riscv64::trap::{init_timer, install_trap_vector, restore_user_context};
use crate::arch::riscv64::vspace::{
    kpptr_to_paddr, make_boot_root_pt, map_user_4k, satp_for, switch_satp, user_flags,
};
use crate::kernel::bootmem;
use crate::object::cap::Cap;
use crate::object::cnode::{cnode_at, cnode_bytes, install_initial_cap};
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

/// Static storage for the rootserver thread's TCB. The first field is a
/// `UserContext`, and `Tcb` is `#[repr(C)]`, so `&ROOTSERVER_TCB` and
/// `&ROOTSERVER_TCB.context` alias the same address — this is how
/// `restore_user_context(&ROOTSERVER_TCB.context)` keeps working while
/// also letting `seL4_TCB_*` invocations against `CAP_INIT_THREAD_TCB`
/// land in the same struct.
#[unsafe(no_mangle)]
pub static mut ROOTSERVER_TCB: Tcb = Tcb::zero();

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
    crate::println!("microkernel: bringing up rootserver");
    crate::println!(
        "  user image: PA [{:#x}, {:#x}) VA offset={:#x} entry={:#x}",
        args.user_pstart,
        args.user_pend,
        args.pv_offset,
        args.user_ventry,
    );

    install_trap_vector();
    init_timer();

    // --- VSpace -----------------------------------------------------------
    let root_pt = make_boot_root_pt();
    crate::println!(
        "  root PT at VA {:#x} PA {:#x}",
        root_pt as usize,
        kpptr_to_paddr(root_pt as usize),
    );

    // Map the rootserver image: PA = VA + pv_offset (elfloader convention).
    let user_va_start = args.user_pstart.wrapping_sub(args.pv_offset);
    let user_va_end = args.user_pend.wrapping_sub(args.pv_offset);
    map_range_4k_identity_from_elfloader(
        root_pt,
        user_va_start,
        user_va_end,
        args.pv_offset,
        user_flags(true, true, true),
    );

    // Allocate + map BootInfo, IPC buffer, user stack.
    let bi_kva = bootmem::alloc_page();
    let bi_pa = kpptr_to_paddr(bi_kva);
    unsafe {
        map_user_4k(
            root_pt,
            USER_BOOTINFO_VA,
            bi_pa,
            user_flags(true, false, false),
        )
    };

    let ipc_kva = bootmem::alloc_page();
    let ipc_pa = kpptr_to_paddr(ipc_kva);
    unsafe {
        map_user_4k(
            root_pt,
            USER_IPC_BUFFER_VA,
            ipc_pa,
            user_flags(true, true, false),
        )
    };

    for i in 0..USER_STACK_PAGES {
        let kva = bootmem::alloc_page();
        let pa = kpptr_to_paddr(kva);
        let va = USER_STACK_TOP - (i + 1) * PAGE_SIZE;
        unsafe { map_user_4k(root_pt, va, pa, user_flags(true, true, false)) };
    }

    // --- Root CNode -------------------------------------------------------
    //
    // 2^13 = 8192 slots * 32 B/cte = 256 KiB. Allocate from the boot pool
    // in 64 contiguous 4 KiB chunks.
    let cnode_pages = cnode_bytes(ROOT_CNODE_SIZE_BITS) / PAGE_SIZE;
    let cnode_base = bootmem::alloc_pages(cnode_pages);
    let cnode_kva = cnode_base as u64;
    let cnode = unsafe { cnode_at(cnode_base as *mut u8, ROOT_CNODE_SIZE_BITS) };

    // Install the 16 fixed initial caps that libsel4 expects at known slots.
    // For M3.1, several of these are "presence stubs" — caps exist with
    // the right tag, but the kernel doesn't honour any invocation on them
    // yet. That's fine for the driver's bootinfo parse and allocator init.
    install_initial_cap(
        cnode,
        bi_slot::CAP_INIT_THREAD_TCB as usize,
        Cap::new_thread(&raw const ROOTSERVER_TCB as u64),
    );
    install_initial_cap(
        cnode,
        bi_slot::CAP_INIT_THREAD_CNODE as usize,
        Cap::new_cnode(
            cnode_kva,
            ROOT_CNODE_SIZE_BITS as u64,
            0,
            64 - ROOT_CNODE_SIZE_BITS as u64,
        ),
    );
    let mut init_vspace_cap = Cap::new_page_table(root_pt as u64);
    init_vspace_cap.set_page_table_mapped_asid(1);
    crate::object::asid::init_root(root_pt as u64);
    install_initial_cap(
        cnode,
        bi_slot::CAP_INIT_THREAD_VSPACE as usize,
        init_vspace_cap,
    );
    install_initial_cap(
        cnode,
        bi_slot::CAP_IRQ_CONTROL as usize,
        Cap::new_irq_control(),
    );
    install_initial_cap(cnode, bi_slot::CAP_DOMAIN as usize, Cap::new_domain());
    install_initial_cap(
        cnode,
        bi_slot::CAP_ASID_CONTROL as usize,
        Cap::new_asid_control(),
    );
    // Initial-thread ASID pool: we don't model the full ASID allocator
    // yet, but the rootserver only needs the cap to exist and accept
    // `ASIDPool_Assign` invocations for spawning child VSpaces. Use a
    // zero base / null pool pointer placeholder.
    install_initial_cap(
        cnode,
        bi_slot::CAP_INIT_THREAD_ASID_POOL as usize,
        Cap::new_asid_pool(0, 0),
    );

    // --- Free memory enumeration → untyped caps --------------------------
    //
    // The rootserver image occupies PA [0x80328000..0x8072e000] (about 4
    // MiB right after our kernel image). Beyond the elfloader's reported
    // user_pend we hand out everything up to the top of QEMU virt's 3 GiB
    // RAM (PA 0x14000_0000). `capPtr` is encoded as the PSpace VA so the
    // 1 GiB megapages we just installed map back to the right PA.
    //
    // We bump the rounded-up free base by an extra 32 MiB safety margin
    // to keep clear of the elfloader's CPIO data (located in PA region
    // 0x8100_0000+ before it copies the kernel/rootserver out).
    const FREE_RAM_BASE_PA: u64 = 0x8200_0000; // 2 MiB aligned, after rootserver + elfloader staging
    const RAM_TOP_PA: u64 = 0x1_4000_0000; // QEMU virt -m 3072 → 3 GiB
    let free_range = FreeRange {
        start_kva: pa_to_pspace_va(FREE_RAM_BASE_PA),
        size: RAM_TOP_PA - FREE_RAM_BASE_PA,
    };

    let mut next_slot = bi_slot::NUM_INITIAL_CAPS as usize;
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

    for (base_kva, bits) in UntypedChunks::new(free_range) {
        if next_slot >= cnode.len() {
            crate::println!("  warn: root CNode full while enumerating untypeds");
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

    // --- Device untypeds (QEMU virt MMIO) --------------------------------
    //
    // The QEMU `virt` board lays out MMIO in [0, 0x80000000) and DRAM
    // starting at 0x80000000. Cover the entire MMIO range with naturally
    // aligned device untypeds so sel4test's "device frame" allocations can
    // pull memory from it.
    //
    // We use PSpace VAs for `capPtr` (sign-extended) — they're never
    // dereferenced by the kernel (device pages aren't readable from the
    // S-mode kernel without an explicit mapping) but they match the
    // encoding the C kernel emits.
    const DEVICE_PA_BASE: u64 = 0x0;
    const DEVICE_PA_TOP: u64 = 0x8000_0000; // exclusive
    let device_range = FreeRange {
        start_kva: pa_to_pspace_va(DEVICE_PA_BASE),
        size: DEVICE_PA_TOP - DEVICE_PA_BASE,
    };
    let device_start_slot = next_slot;
    for (base_kva, bits) in UntypedChunks::new(device_range) {
        if next_slot >= cnode.len() || bi_untyped_count >= MAX_NUM_BOOTINFO_UNTYPED_CAPS {
            break;
        }
        let cap = make_untyped_cap(base_kva, bits, true);
        install_initial_cap(cnode, next_slot, cap);
        // For device caps, paddr = kva - PPTR_BASE.
        let pa = base_kva - (crate::abi::constants::PPTR_BASE as u64);
        untyped_list_local[bi_untyped_count] = UntypedDesc {
            paddr: pa,
            size_bits: bits,
            is_device: 1,
            _padding: [0; 6],
        };
        next_slot += 1;
        bi_untyped_count += 1;
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
    let user_va_start_aligned = args.user_pstart.wrapping_sub(args.pv_offset) & !(PAGE_SIZE - 1);
    let user_va_end_aligned =
        (args.user_pend.wrapping_sub(args.pv_offset) + PAGE_SIZE - 1) & !(PAGE_SIZE - 1);
    let mut va = user_va_start_aligned;
    while va < user_va_end_aligned {
        if next_slot >= cnode.len() {
            crate::println!("  warn: root CNode full while installing user-image frame caps");
            break;
        }
        let pa = va.wrapping_add(args.pv_offset) as u64;
        let frame_kva = pa_to_pspace_va(pa);
        let mut cap = Cap::new_frame(frame_kva, 0 /* 4 KiB */, 2 /* RW */, false);
        cap.set_frame_mapped_addr(va as u64);
        install_initial_cap(cnode, next_slot, cap);
        next_slot += 1;
        va += PAGE_SIZE;
    }
    let user_image_frames_end = next_slot;
    crate::println!(
        "  user image frames: slots {}..{} ({} caps)",
        user_image_frames_start,
        user_image_frames_end,
        user_image_frames_end - user_image_frames_start,
    );
    crate::println!(
        "  device untyped: slots {}..{} ({} caps)",
        device_start_slot,
        device_end_slot,
        device_end_slot - device_start_slot,
    );
    crate::println!(
        "  root CNode: {} initial caps, {} untyped (slots {}..{}), {} slots free",
        bi_slot::NUM_INITIAL_CAPS,
        bi_untyped_count,
        untyped_start_slot,
        untyped_end_slot,
        cnode.len() - next_slot,
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
        ipc_kva as *mut u64,
        USER_IPC_BUFFER_VA as u64,
        root_pt as u64,
    );
    // Mirror the same data into ROOTSERVER_TCB so that helper TCBs
    // and the rootserver speak the same Cap-of-CSpace / VSpace / IPC
    // language. Once thread::current() learns to follow tcb::current()
    // (in this iteration), every cap-lookup and IPC-buffer access in
    // the syscall path will draw from these fields.
    unsafe {
        let rs = &raw mut ROOTSERVER_TCB;
        (*rs).cspace_cap = cnode_cap_for_thread;
        (*rs).vspace_cap = init_vspace_cap;
        (*rs).ipc_buffer_uva = USER_IPC_BUFFER_VA as u64;
        (*rs).ipc_buffer_kva = ipc_kva as u64;
        // The IPC-buffer Frame cap isn't required by `do_recv`'s MR
        // synthesis (which walks via `ipc_buffer_kva`), so we leave
        // ipc_buffer_cap as `null` for the rootserver.
    }

    // --- Populate BootInfo -----------------------------------------------
    let bi = bi_kva as *mut BootInfo;
    unsafe {
        ptr::write_bytes(bi as *mut u8, 0, core::mem::size_of::<BootInfo>());
        (*bi).node_id = 0;
        (*bi).num_nodes = 1;
        (*bi).num_io_pt_levels = 0;
        (*bi).ipc_buffer = USER_IPC_BUFFER_VA as u64;
        (*bi).empty = SlotRegion {
            start: next_slot as u64,
            end: cnode.len() as u64,
        };
        (*bi).user_image_frames = SlotRegion {
            start: user_image_frames_start as u64,
            end: user_image_frames_end as u64,
        };
        (*bi).user_image_paging = SlotRegion { start: 0, end: 0 };
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

    crate::println!(
        "  bootinfo: ipc@{:#x} cnode_bits={} untyped=[{}..{}) ({} caps)",
        USER_IPC_BUFFER_VA,
        ROOT_CNODE_SIZE_BITS,
        untyped_start_slot,
        untyped_end_slot,
        bi_untyped_count,
    );

    // --- Switch to user VSpace and sret ----------------------------------
    let satp = satp_for(root_pt, 1);
    crate::println!("  satp <- {:#x}", satp);
    unsafe { switch_satp(satp) };
    crate::machine::plic::init();

    unsafe {
        let t = &raw mut ROOTSERVER_TCB;
        // sstatus: SPIE=1 (sret re-enables interrupts),
        //          SUM=1  (kernel can touch user memory),
        //          SPP=0  (sret enters U-mode).
        (*t).context.pc = args.user_ventry as u64;
        (*t).context.sstatus = crate::arch::riscv64::trap::ROOTSERVER_SSTATUS;
        (*t).context.regs[10] = USER_BOOTINFO_VA as u64; // a0 = bootinfo
        (*t).context.regs[11] = 0;
        (*t).context.regs[2] = USER_STACK_TOP as u64; // sp
        (*t).state = crate::object::tcb::ThreadState::Running as u8;
        (*t).priority = 255;
        (*t).mcp = 255;
        (*t).time_slice_ticks = tcb::DEFAULT_TIME_SLICE_TICKS;
        tcb::set_current(t);
        // Seed the scheduler's runqueue with the rootserver, so
        // `schedule()` always has a runnable TCB to return.
        tcb::enqueue(t);
    }

    crate::println!("  entering user mode at {:#x}", args.user_ventry);
    crate::println!("  --- transferring control to rootserver ---");
    unsafe {
        let uc = &raw mut (*&raw mut ROOTSERVER_TCB).context;
        restore_user_context(uc);
    }
}

/// Map a contiguous VA range of the user image to its PA range. Both VAs
/// and PAs are required to be 4 KiB aligned; the caller passes the
/// elfloader's `pv_offset` to recover PA from VA (PA = VA + pv_offset).
fn map_range_4k_identity_from_elfloader(
    root: *mut PageTable,
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
        unsafe { map_user_4k(root, va, pa, flags) };
        va += PAGE_SIZE;
    }
}
