//! High-level kernel boot path (M2.2): set up the user image's VSpace,
//! construct a minimal `seL4_BootInfo`, and `sret` into the root task.
//!
//! We are intentionally not implementing CSpace / TCB / capability bookkeeping
//! yet — those come in M3. The goal of M2.2 is just to get sel4test-driver
//! into user mode far enough to print something via `seL4_DebugPutChar`.

use core::ptr;

use crate::abi::bootinfo::{BootInfo, SlotRegion, UntypedDesc};
use crate::abi::constants::{MAX_NUM_BOOTINFO_UNTYPED_CAPS, ROOT_CNODE_SIZE_BITS};
use crate::arch::riscv64::sv39::{PAGE_SIZE, PageTable};
use crate::arch::riscv64::trap::{install_trap_vector, restore_user_context, UserContext};
use crate::arch::riscv64::vspace::{
    kpptr_to_paddr, make_boot_root_pt, map_user_4k, satp_for, switch_satp, user_flags,
};
use crate::kernel::bootmem;

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

/// Bootstrap the user environment and drop into U-mode.
pub fn bringup_rootserver(args: &BootArgs) -> ! {
    crate::println!("microkernel: bringing up rootserver");

    install_trap_vector();

    // Build the rootserver's VSpace from scratch.
    let root_pt = make_boot_root_pt();
    crate::println!(
        "  root PT at VA {:#x} PA {:#x}",
        root_pt as usize,
        kpptr_to_paddr(root_pt as usize),
    );

    // Map the rootserver image. The elfloader already loaded it into
    // physical memory at [user_pstart..user_pend), and reported the
    // pv_offset (PA = VA + pv_offset) and entry VA. We need to walk the
    // VA range and install user-readable/executable/writable pages for
    // each 4 KiB frame.
    let user_va_start = args.user_pstart.wrapping_sub(args.pv_offset);
    let user_va_end = args.user_pend.wrapping_sub(args.pv_offset);
    map_range_4k_identity_from_elfloader(
        root_pt,
        user_va_start,
        user_va_end,
        args.pv_offset,
        user_flags(true, true, true),
    );
    crate::println!(
        "  mapped user image VA [{:#x}..{:#x}) PA [{:#x}..{:#x})",
        user_va_start,
        user_va_end,
        args.user_pstart,
        args.user_pend
    );

    // Allocate + map the BootInfo frame.
    let bi_kva = bootmem::alloc_page();
    let bi_pa = kpptr_to_paddr(bi_kva);
    unsafe { map_user_4k(root_pt, USER_BOOTINFO_VA, bi_pa, user_flags(true, false, false)) };

    // Allocate + map the IPC buffer frame.
    let ipc_kva = bootmem::alloc_page();
    let ipc_pa = kpptr_to_paddr(ipc_kva);
    unsafe { map_user_4k(root_pt, USER_IPC_BUFFER_VA, ipc_pa, user_flags(true, true, false)) };

    // Allocate + map a small user stack.
    for i in 0..USER_STACK_PAGES {
        let kva = bootmem::alloc_page();
        let pa = kpptr_to_paddr(kva);
        let va = USER_STACK_TOP - (i + 1) * PAGE_SIZE;
        unsafe { map_user_4k(root_pt, va, pa, user_flags(true, true, false)) };
    }

    crate::println!(
        "  bootinfo VA {:#x} (PA {:#x}), ipc buffer VA {:#x} (PA {:#x})",
        USER_BOOTINFO_VA,
        bi_pa,
        USER_IPC_BUFFER_VA,
        ipc_pa,
    );

    // Populate BootInfo with safe defaults.
    let bi = bi_kva as *mut BootInfo;
    unsafe {
        ptr::write_bytes(bi as *mut u8, 0, core::mem::size_of::<BootInfo>());
        (*bi).node_id = 0;
        (*bi).num_nodes = 1;
        (*bi).num_io_pt_levels = 0;
        (*bi).ipc_buffer = USER_IPC_BUFFER_VA as u64;
        (*bi).empty = SlotRegion { start: 0, end: 0 };
        (*bi).shared_frames = SlotRegion { start: 0, end: 0 };
        (*bi).user_image_frames = SlotRegion { start: 0, end: 0 };
        (*bi).user_image_paging = SlotRegion { start: 0, end: 0 };
        (*bi).io_space_caps = SlotRegion { start: 0, end: 0 };
        (*bi).extra_bi_pages = SlotRegion { start: 0, end: 0 };
        (*bi).init_thread_cnode_size_bits = ROOT_CNODE_SIZE_BITS as u64;
        (*bi).init_thread_domain = 0;
        (*bi).untyped = SlotRegion { start: 0, end: 0 };
        for u in (*bi).untyped_list.iter_mut() {
            *u = UntypedDesc::default();
        }
        let _ = MAX_NUM_BOOTINFO_UNTYPED_CAPS; // suppress unused
    }

    // Switch to the rootserver's VSpace.
    let satp = satp_for(root_pt, 1 /* ASID */);
    crate::println!("  satp <- {:#x}", satp);
    unsafe { switch_satp(satp) };

    // Prepare the rootserver's UserContext: pc = user_ventry, sp = stack top,
    // a0 = bootinfo pointer (sel4runtime convention).
    unsafe {
        let uc = &raw mut ROOTSERVER_CONTEXT;
        (*uc).pc = args.user_ventry as u64;
        // RISC-V kernel-mode sstatus to enter user-mode via sret:
        //   SPP=0 (return to U), SPIE=1 (enable interrupts after sret),
        //   SUM=1 (allow S to read U pages; helpful for debugging).
        // We compose the bits directly:
        //   bit 5: SPIE = 1
        //   bit 8: SPP  = 0 (cleared)
        //   bit 18: SUM = 1
        (*uc).sstatus = (1 << 5) | (1 << 18);
        // x10 = a0
        (*uc).regs[10] = USER_BOOTINFO_VA as u64;
        // x2  = sp
        (*uc).regs[2] = USER_STACK_TOP as u64;
        // x3 (gp) is set by the user's _sel4_start preamble.
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
