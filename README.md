# microkernel — a Rust reimplementation of seL4 (RV64, qemu-riscv-virt)

A minimal, milestone-driven rewrite of the seL4 microkernel in Rust, targeting
the same ABI as the official C kernel so that the existing `sel4test-driver`
binary boots unmodified on top of it.

## Current status

| Milestone | Description | Status |
|-----------|-------------|--------|
| M0 | Build skeleton, no_std ELF cross-compiles | ✅ Done |
| M1 | M-mode standalone boot, NS16550 UART banner via `qemu -bios none -kernel` | ✅ Done |
| M2.1 | S-mode boot under the seL4 C elfloader, SBI console, prints kernel banner | ✅ Done |
| M2.2 | `tools/pack-image.sh` re-packs the official image; sel4test-driver enters U-mode and prints via `seL4_DebugPutChar` | ✅ Done |
| M3   | CSpace / CTE / MDB, capability ops, TCBs, Endpoints, VSpace `Map/Unmap`, Untyped `Retype` | 🚧 In progress |
| M4   | PLIC IRQs, SBI timer / preemption, debug breakpoints, full sel4test pass | ⏳ Not started |

A live run of M2.2 (truncated, full output is reproducible via `make run`):

```text
ELF-loader started on (HART 0) (NODES 1)
  ...
ELF-loading image 'kernel' to 80200000
  paddr=[80200000..80324fff]
  vaddr=[ffffffff80200000..ffffffff80324fff]
ELF-loading image 'rootserver' to 80327000
  paddr=[80327000..8072cfff]
  vaddr=[10000..415fff]
Jumping to kernel-image entry point...

microkernel: Rust kernel booted (S-mode, Sv39)
  hart_id=0 core_id=0 dtb=0x80325000 (5227 bytes)
  user image: pa=[0x80327000..0x8072d000], pv_offset=0x80317000, entry=0x1c6cc
microkernel: bringing up rootserver
  root PT at VA 0xffffffff80204000 PA 0x80204000
  mapped user image VA [0x10000..0x416000) ...
  satp <- 0x8000100000080204
  entering user mode at 0x1c6cc
  --- transferring control to rootserver ---

Warning: using printf before serial is set up. This only works as your
printf is backed by seL4_Debug_PutChar()
vka_alloc_object_at_maybe_dev@object.h:57 ...   <-- M3 work starts here
```

## Repository layout

```
microkernel/
├── flake.nix              # Nix dev shell: Rust + RISC-V toolchain + qemu/ninja/cpio
├── .envrc                 # `use flake` for direnv
├── rust-toolchain.toml    # stable + riscv64gc-unknown-none-elf
├── Cargo.toml             # workspace
├── .cargo/config.toml     # build-target + rustflags
├── kernel/
│   ├── Cargo.toml
│   ├── linker.ld          # KERNEL_ELF_BASE=0xFFFFFFFF80200000, LMA 0x80200000
│   └── src/
│       ├── main.rs        # entry, panic handler
│       ├── print.rs       # println! macros via machine::console
│       ├── abi/           # byte-exact seL4 ABI mirror
│       │   ├── constants.rs
│       │   ├── syscall.rs
│       │   ├── types.rs       # MessageInfo, CapRights, CNodeCapData
│       │   └── bootinfo.rs    # seL4_BootInfo, IPCBuffer
│       ├── arch/riscv64/
│       │   ├── boot.rs        # _start + init_kernel
│       │   ├── csr.rs         # S-mode CSR accessors
│       │   ├── sbi.rs         # legacy SBI ecall wrappers
│       │   ├── sv39.rs        # PageTable / PTE / make_satp
│       │   ├── vspace.rs      # kpptr<->paddr, map_user_4k, make_boot_root_pt
│       │   ├── trap.S         # asm trap entry / restore_user_context
│       │   └── trap.rs        # UserContext + handle_trap_rust
│       ├── machine/
│       │   ├── console.rs     # SBI-backed putc
│       │   └── uart.rs        # NS16550 (M1 only)
│       └── kernel/
│           ├── boot.rs        # bringup_rootserver
│           └── bootmem.rs     # bump page allocator
└── tools/
    ├── pack-image.sh      # rebuild Rust kernel + ninja repackage + emit image
    └── simulate.sh        # qemu wrapper (standalone or packed image)
```

## Quick start

Requires Nix with flakes enabled, plus the upstream seL4 source already
built at `${SEL4_BUILD_DIR:-/Users/wangfiox/sel4/sel4test/build-riscv64}`.

```sh
direnv allow                  # or: nix develop

# Build the Rust kernel:
cargo build --release

# Pack our kernel into a sel4test-driver image and boot it under QEMU:
./tools/pack-image.sh
./tools/simulate.sh           # Ctrl-A x to exit

# Boot the standalone M1 banner (no elfloader, M-mode):
MODE=standalone ./tools/simulate.sh
```

## Key ABI / layout constants (frozen against upstream)

| Symbol | Value | Source |
|--------|-------|--------|
| `PHYS_BASE_RAW` | `0x80200000` | `build-riscv64/kernel/gen_headers/plat/machine/devices_gen.h` |
| `PPTR_BASE` | `0xFFFFFFC0_00000000` | `kernel/include/arch/riscv/arch/64/mode/hardware.h` |
| `PPTR_TOP`  | `0xFFFFFFFF_80000000` | ditto |
| `KERNEL_ELF_BASE` | `0xFFFFFFFF_80200000` | `PPTR_TOP + (PHYS_BASE_RAW & MASK(30))` |
| `seL4_PageBits` | 12 | RV64 ABI |
| `seL4_TCBBits` | 11 (= 2 KiB) | RV64 ABI |
| `seL4_SlotBits` | 5 (= 32 B / cte) | RV64 ABI |
| `CONFIG_ROOT_CNODE_SIZE_BITS` | 13 (= 8192 slots) | gen_config.h |
| `CONFIG_MAX_NUM_BOOTINFO_UNTYPED_CAPS` | 230 | gen_config.h |
| `CONFIG_NUM_DOMAINS` | 1 | gen_config.h |
| `CONFIG_NUM_PRIORITIES` | 256 | gen_config.h |
| `CONFIG_KERNEL_MCS` | disabled | gen_config.h |
| `CONFIG_PT_LEVELS` | 3 (Sv39) | gen_config.h |

## Boot flow (M2.2)

```
QEMU virt
  └─> OpenSBI fw_payload.elf  (M-mode firmware bundled with elfloader)
        └─> seL4 elfloader     (sets up Sv39, loads kernel.elf + sel4test-driver)
              └─> Rust kernel _start (.boot.text @ 0xFFFFFFFF80200000)
                    └─> init_kernel(a0..a7) [boot.rs]
                          └─> kernel::boot::bringup_rootserver
                                ├─ install_trap_vector()
                                ├─ make_boot_root_pt()                  (1 GiB megapage for kernel ELF window)
                                ├─ map_user_4k(...) for sel4test-driver
                                ├─ alloc + map BootInfo frame           (VA 0x7FFFE000)
                                ├─ alloc + map IPC buffer frame         (VA 0x7FFFF000)
                                ├─ alloc + map 64 KiB user stack
                                ├─ populate seL4_BootInfo (mostly zeros)
                                ├─ switch_satp(satp_for(root, ASID=1))
                                └─ restore_user_context(&ROOTSERVER_CONTEXT)
                                      └─ sret → _sel4_start in U-mode
```

## Next steps (M3)

The driver currently aborts because the BootInfo we hand it has no
untypeds and no real CSpace. Concrete next pieces, in order:

1. **CSpace / CTE / cap encoding.** `cte_t` is two words on RV64; the
   second word holds MDB pointers. Layout in
   `kernel/generated/arch/object/structures_gen.h`.
2. **Root CNode at `seL4_CapInitThreadCNode`** populated with the
   `seL4_NumInitialCaps = 16` fixed slots.
3. **Initial untypeds** carved from the physical memory range reported by
   the DTB minus the kernel image, IPC buffer, BootInfo frame, root PT
   pool, and user image.
4. **TCB object** (`seL4_TCB`) for the initial thread; thread_state /
   `Restart` / `Inactive`.
5. **VSpace caps** for the user's L1/L2/L0 page tables.
6. **`seL4_Untyped_Retype`** — the workhorse syscall that lets the driver
   build all other objects.

The shape of `init_kernel` will follow the C kernel's `create_root_cnode`,
`create_initial_thread`, `create_bi_frame`, `create_untypeds`, then
`activate_initial_thread` (which is essentially our existing
`restore_user_context`).
