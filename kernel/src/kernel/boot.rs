//! High-level kernel boot path (M2.2): set up the user image's VSpace,
//! construct a minimal `seL4_BootInfo`, and `sret` into the root task.
//!
//! We are intentionally not implementing CSpace / TCB / capability bookkeeping
//! yet — those come in M3. The goal of M2.2 is just to get sel4test-driver
//! into user mode far enough to print something via `seL4_DebugPutChar`.

use core::ptr;

use crate::abi::bootinfo::{
    self as bi_slot, BootInfo, SlotRegion, UntypedDesc,
};
use crate::abi::constants::{
    KERNEL_ELF_BASE, MAX_NUM_BOOTINFO_UNTYPED_CAPS, ROOT_CNODE_SIZE_BITS,
    SEL4_MAX_UNTYPED_BITS, SEL4_MIN_UNTYPED_BITS, SEL4_SLOT_BITS,
};
use crate::arch::riscv64::sv39::{PAGE_SIZE, PageTable};
use crate::arch::riscv64::trap::{install_trap_vector, restore_user_context, UserContext};
use crate::arch::riscv64::vspace::{
    kpptr_to_paddr, make_boot_root_pt, map_user_4k, satp_for, switch_satp, user_flags,
};
use crate::kernel::bootmem;
use crate::object::cap::Cap;
use crate::object::cnode::{cnode_at, cnode_bytes, install_initial_cap};
use crate::object::untyped::{make_untyped_cap, FreeRange, UntypedChunks};

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

/// Static storage for the rootserver's `UserContext`. Saved by `trap_entry`
/// when the user thread traps; restored by `restore_user_context`.
#[unsafe(no_mangle)]
pub static mut ROOTSERVER_CONTEXT: UserContext = UserContext {
    regs: [0; 32],
    pc: 0,
    sstatus: 0,
    _reserved: 0,
};

/// Translate a kernel-ELF-window virtual address to a physical address.
/// Provided here so we can pass it to other modules without exposing the
/// raw arithmetic.
#[inline]
fn kva_to_pa(kva: u64) -> u64 {
    use crate::abi::constants::PHYS_BASE_RAW;
    kva - (KERNEL_ELF_BASE as u64) + (PHYS_BASE_RAW as u64)
}

/// Bootstrap the user environment and drop into U-mode.
pub fn bringup_rootserver(args: &BootArgs) -> ! {
    crate::println!("microkernel: bringing up rootserver");

    install_trap_vector();

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
    unsafe { map_user_4k(root_pt, USER_BOOTINFO_VA, bi_pa, user_flags(true, false, false)) };

    let ipc_kva = bootmem::alloc_page();
    let ipc_pa = kpptr_to_paddr(ipc_kva);
    unsafe { map_user_4k(root_pt, USER_IPC_BUFFER_VA, ipc_pa, user_flags(true, true, false)) };

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
        bi_slot::CAP_INIT_THREAD_CNODE as usize,
        Cap::new_cnode(cnode_kva, ROOT_CNODE_SIZE_BITS as u64, 0, 64 - ROOT_CNODE_SIZE_BITS as u64),
    );
    install_initial_cap(
        cnode,
        bi_slot::CAP_INIT_THREAD_VSPACE as usize,
        Cap::new_page_table(root_pt as u64),
    );
    install_initial_cap(
        cnode,
        bi_slot::CAP_IRQ_CONTROL as usize,
        Cap::new_irq_control(),
    );
    install_initial_cap(
        cnode,
        bi_slot::CAP_DOMAIN as usize,
        Cap::new_domain(),
    );

    // --- Free memory enumeration → untyped caps --------------------------
    //
    // For M3 we keep this dumb: take a hardcoded 64 MiB region above the
    // rootserver image and let the splitter chop it into naturally-aligned
    // power-of-two chunks. The DTB-driven layout discovery is M4 work.
    const FREE_RAM_BASE_PA: u64 = 0x8100_0000;
    const FREE_RAM_BYTES: u64 = 64 * 1024 * 1024;
    let free_range = FreeRange {
        // capPtr encodes a kernel-window VA. Use the kernel ELF window
        // since the elfloader-set megapage already covers PA up to 0xC0000000.
        start_kva: (KERNEL_ELF_BASE as u64) + (FREE_RAM_BASE_PA - 0x8000_0000),
        size: FREE_RAM_BYTES,
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
    }; MAX_NUM_BOOTINFO_UNTYPED_CAPS];

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
    let untyped_end_slot = next_slot;
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
        (*bi).user_image_frames = SlotRegion { start: 0, end: 0 };
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

    unsafe {
        let uc = &raw mut ROOTSERVER_CONTEXT;
        (*uc).pc = args.user_ventry as u64;
        // sstatus: SPIE=1 (bit 5), SUM=1 (bit 18), SPP=0
        (*uc).sstatus = (1 << 5) | (1 << 18);
        (*uc).regs[10] = USER_BOOTINFO_VA as u64; // a0 = bootinfo
        (*uc).regs[2] = USER_STACK_TOP as u64;    // sp
    }

    crate::println!("  entering user mode at {:#x}", args.user_ventry);
    crate::println!("  --- transferring control to rootserver ---");
    unsafe {
        let uc = &raw mut ROOTSERVER_CONTEXT;
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
