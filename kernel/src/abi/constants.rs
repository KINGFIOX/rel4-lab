//! Compile-time constants frozen against the upstream
//! `qemu-riscv-virt` build of the official seL4 kernel.
//!
//! Pulled from `build-riscv64/kernel/gen_config/kernel/gen_config.h` and
//! `kernel/libsel4/sel4_arch_include/riscv64/sel4/sel4_arch/constants.h`.

#![allow(dead_code)]

// ---- General kernel config ------------------------------------------------

pub const WORD_SIZE_BITS: usize = 6; // log2(64)
pub const WORD_BITS: usize = 1 << WORD_SIZE_BITS;
pub const WORD_BYTES: usize = WORD_BITS / 8;

pub const NUM_DOMAINS: usize = 1;
pub const NUM_PRIORITIES: usize = 256;
pub const ROOT_CNODE_SIZE_BITS: usize = 13;
pub const MAX_NUM_NODES: usize = 1;
pub const KERNEL_STACK_BITS: usize = 12;
pub const MAX_NUM_BOOTINFO_UNTYPED_CAPS: usize = 230;
pub const TIME_SLICE_TICKS: usize = 5;
pub const TIMER_TICK_MS: usize = 2;
pub const RETYPE_FAN_OUT_LIMIT: usize = 256;
pub const RESET_CHUNK_BITS: usize = 8;

// ---- seL4 object size bits (RV64) ----------------------------------------

pub const SEL4_PAGE_BITS: usize = 12; // 4 KiB page
pub const SEL4_LARGE_PAGE_BITS: usize = 21; // 2 MiB
pub const SEL4_HUGE_PAGE_BITS: usize = 30; // 1 GiB
pub const SEL4_PAGE_TABLE_BITS: usize = 12; // 4 KiB PT object
pub const SEL4_PAGE_TABLE_ENTRIES: usize = 512;

pub const SEL4_SLOT_BITS: usize = 5; // sizeof(cte_t) == 32
pub const SEL4_TCB_BITS: usize = 11; // 2 KiB TCB
pub const SEL4_ENDPOINT_BITS: usize = 4;
pub const SEL4_NOTIFICATION_BITS: usize = 5;
pub const SEL4_REPLY_BITS: usize = 4;
pub const SEL4_ASID_POOL_BITS: usize = 12;

pub const SEL4_MIN_UNTYPED_BITS: usize = 4;
pub const SEL4_MAX_UNTYPED_BITS: usize = 38;

// ---- IPC ------------------------------------------------------------------

/// Number of IPC message registers transferred in physical registers
/// (a2..a5 on RISC-V64). Anything past this lives in the IPC buffer.
pub const N_MSG_REGISTERS: usize = 4;
/// Total number of message registers usable per IPC.
pub const N_TOTAL_MSG_REGISTERS: usize = 120;

// ---- Architecture ---------------------------------------------------------

pub const PT_INDEX_BITS: usize = 9; // 512 entries per level (Sv39)
pub const PT_LEVELS: usize = 3; // Sv39
pub const RISCV_PG_SHIFT: usize = 12;

pub const PHYS_BASE_RAW: usize = 0x8020_0000;
pub const PPTR_BASE: usize = 0xFFFFFFC0_00000000;
pub const PPTR_TOP: usize = 0xFFFFFFFF_80000000;
pub const KERNEL_ELF_BASE: usize = PPTR_TOP + (PHYS_BASE_RAW & ((1usize << 30) - 1));
// PA = VA - (PPTR_TOP - PADDR_BASE) when kernel-window-mapped:
//   PADDR_BASE = 0 ⇒ kernel window maps VA[PPTR_BASE..PPTR_TOP) → PA[0..2^38)
pub const PADDR_BASE: usize = 0;
