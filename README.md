# microkernel — a Rust reimplementation of seL4 (RV64, qemu-riscv-virt)

A minimal, milestone-driven rewrite of the seL4 microkernel in Rust, targeting
the same ABI as the official C kernel so that the existing `sel4test-driver`
binary boots unmodified on top of it.

## Current status

The `sel4test-driver` rootserver boots, spawns helper TCBs and per-test
child processes in their own VSpaces, and runs them on the Rust kernel
through the official `libsel4` ABI. Endpoint IPC, notifications, reply
caps, FPU save/restore, timer preemption, several CNode/Untyped paths,
multi-size frame map/unmap, DomainSet, fault IPC, and ASID pool creation
are implemented far enough for the full suite to run to completion.

Latest verified checkpoint: `./tools/run-tests.sh` passes:
**116/116 enabled tests passing, 51 upstream-disabled tests remaining**.
The final push from 91/116 fixed two seL4 semantics gaps:
`CNode_Delete` now uses C-kernel-style `emptySlot` MDB splicing instead
of rejecting slots with CDT children, which clears the `RETYPE0000..0002`
teardown failures and the `VSPACE0006` ASID stress exhaustion; endpoint
IPC now supports the pre-MCS single receive-slot cap-transfer path, which
lets the serial server receive client shared-memory frame caps and clears
all `SERSERV_*` failures. The current M4.4 branch also has a first
minimal IRQ path: `IRQControl_Get` can issue `IRQHandler` caps,
`IRQHandler_SetNotification/Clear/Ack` dispatches through the real ABI
labels, handler caps participate in the MDB, and the synthetic SBI timer
IRQ can signal a bound notification. This is intentionally not a full
PLIC implementation yet, so the 51 upstream-disabled tests remain
disabled for now.

| Milestone | Description | Status |
|-----------|-------------|--------|
| M0 | Build skeleton, no_std ELF cross-compiles | ✅ Done |
| M1 | M-mode standalone boot, NS16550 UART banner via `qemu -bios none -kernel` | ✅ Done |
| M2.1 | S-mode boot under the seL4 C elfloader, SBI console, prints kernel banner | ✅ Done |
| M2.2 | `tools/pack-image.sh` re-packs the official image; sel4test-driver enters U-mode and prints via `seL4_DebugPutChar` | ✅ Done |
| M3.1 | `cap_t` + `mdb_node_t` + `cte_t`, root CNode with 16 fixed initial caps, untyped enumeration into BootInfo | ✅ Done |
| M3.2 | `seL4_Call` slow path: CSpace lookup, extra-cap reading from IPCBuffer, error encoding | ✅ Done |
| M3.3 | `Untyped_Retype` (Untyped/CNode/Frame/PageTable/TCB/EP/Notification), `RISCVPage_Map`, `RISCVPageTable_Map` | ✅ Done |
| M3.4 | CNode `Copy/Mint/Move/Mutate/Delete/Revoke` + MDB CDT linkage | ✅ Done |
| M3.5 | PSpace window (8 × 1 GiB megapages), 3 GiB RAM as untypeds, QEMU MMIO as device untypeds, `seL4_DebugCapIdentify` returns real cap tags | ✅ Done |
| M3.5.1 | CDT correctness: initial-cap and Retype-created MDB nodes carry the right `revocable / firstBadged` bits (matches `write_slot` + `isCapRevocable` in C). Without this, `Untyped_Retype` could reset `free_index` while a sibling carving was still live → classic use-after-free. | ✅ Done |
| M3.5.2 | `isCapRevocable(newCap, srcCap)` on Copy/Mint: untyped/EP-badge/Ntfn-badge/IRQ-handler copies are revocable *roots* of their own derivation subtree. Fixed Revoke walking past `BIG_UT → COPY → sub_ut` when `COPY.revocable` was incorrectly false. | ✅ Done |
| M3.5.3 | `finalize_cap(CNode)` empties the slab when a CNode is being torn down (per the C `finaliseCap` Zombie path). Necessary to recycle a test process's CNode-backed Untyped memory cleanly. | ✅ Done |
| M3.6 | Minimal Notification (state + badge) + `seL4_Send`/`seL4_Recv` slow-path dispatch — enough to make `BIND00xx`, `SYNC00xx`, `CANCEL_BADGED_SENDS` pass. | ✅ Done |
| M3.7 | Minimal ASID table: every Frame cap records the ASID of the VSpace it's mapped into so `Page_Unmap` + `finalize_cap(Frame)` rip the leaf PTE out of the *right* root PT during cross-vspace Revoke. | ✅ Done |
| M3.8 | `BootInfo.userImageFrames` populated with Frame caps for the rootserver ELF range, so `libsel4utils` doesn't re-allocate VAs over the driver's own image. | ✅ Done |
| M3.9 | Full enabled sel4test suite passes: **116/116 enabled tests pass; 51 tests disabled upstream**. | ✅ Done |
| M4.1 | Recycle PT pages on `unmap_user_4k` — empty L1/L0 tables go straight back onto `BOOT_PT_FREELIST`, so the 128-page static pool sustains the whole 116-test sweep. | ✅ Done |
| M4.2a | `Tcb` struct + per-Untyped-Retype slab init + dedicated `handle_thread()` for all 15 non-MCS `TCB_*` labels (Configure/SetSpace/SetIPCBuffer/SetPriority/SetMCPriority/SetSchedParams/WriteRegisters/ReadRegisters/CopyRegisters/Suspend/Resume/BindNotification/UnbindNotification/SetTLSBase/SetFlags). Data is parsed, validated, and persisted into the TCB slab. | ✅ Done |
| M4.2b | Rootserver runs out of a real `Tcb` (`ROOTSERVER_TCB` in BSS); `CAP_INIT_THREAD_TCB` installed; `tcb::CURRENT_TCB` tracked. `restore_user_context` now restores from `current_tcb()->context`, so any `seL4_TCB_*` write against the rootserver TCB (`SetTLSBase`, future `WriteRegisters`, …) takes effect on next sret. | ✅ Done |
| M4.2c | 256-bin per-priority ready queue (`RUNQUEUES` + 4-word `READY_BITMAP` for O(1) "highest set priority" scan), `enqueue/dequeue/schedule()` primitives, `kernel_exit()` hook called from every trap return. `TCB_Resume`/`Suspend` move the TCB in/out of the queue; `TCB_WriteRegisters(resume_target=1)` (the real "start helper" call) hits the same path. `seL4_Yield` rotates within the priority bin. Trampoline now takes the next TCB's `UserContext*` straight out of `handle_trap_rust`'s return value. | ✅ Done |
| M4.2d | `Endpoint` struct (16 bytes, 2-bit state packed in head ptr, doubly-linked wait list reusing `Tcb.queue_{next,prev}`), `enqueue_waiter / pop_head / remove_waiter / finalize` primitives, init hook on `Untyped_Retype(Endpoint)`, `finalize_cap(Endpoint)` wakes all blocked waiters back into the runqueue. `Tcb.caller` field added for the pre-MCS Call/Reply pattern. | ✅ Done |
| M4.2e | Wire `do_send` / `do_recv` / `do_call` / `do_reply` to the `Endpoint` state machine + `tcb::set_current → refresh_from_tcb` so syscalls read MRs from the *running* TCB's IPC buffer. The rootserver actually blocks on its fault EP now and the child test process gets scheduled in. | ✅ Done |
| M4.2e+ | `kernel_exit` writes `satp` + `sfence.vma` when the next TCB lives in a different VSpace; new user root PTs (Untyped → PageTable) get the kernel-ELF + PSpace megapage entries copied in (`copy_kernel_mappings_to`) so traps from U-mode can still reach `trap_entry`. | ✅ Done |
| M4.2e+ | `Page_Map` now parses `seL4_CapRights_t` (bit 0 W, bit 1 R) and the RISC-V VM-attr `riscvExecuteNever` bit instead of hard-coding `R/W/¬X`. ELF code pages are correctly mapped executable. | ✅ Done |
| M4.2e+ | `TCB_Configure` / `TCB_SetSpace` apply the `seL4_CNode_CapData` word (guard ‖ guard_size) to the cspace cap before storing — without this the child process's root CNode could only resolve cptrs equal to its own bits, and every libsel4allocman retype came back `IllegalOperation`. | ✅ Done |
| M4.2f | Close the final enabled-suite gaps: CNode Delete follows `cteDelete(..., exposed=true)` / `emptySlot` semantics, and IPC cap transfer handles the single receive-slot path used by serial-server shared memory setup. | ✅ Done |
| M4.3 | VM/cap/user fault forwarding to the configured fault endpoint; `PAGEFAULT0001..0005` and `PAGEFAULT1001..1004` pass. | ✅ Done |
| M4.4a | Minimal IRQControl/IRQHandler ABI support: issue one handler cap per IRQ, derive it under IRQControl in the MDB, bind/clear Notification caps, finalize handler state on last delete, and signal the kernel timer IRQ notification from the SBI timer trap. `Ack` is accepted as a no-op and RISC-V trigger configuration is parsed but not programmed. | ✅ Done |
| M4.4 | Full PLIC IRQ chain, userspace timer enablement, and debug breakpoints (unlocks the 51 disabled tests) | ⏳ Pending |

A live run (kernel boots, the rootserver's `allocman` carves up untyped
memory via dozens of `Untyped_Retype` calls, maps frames via
`RISCVPage_Map`, brings up the serial server, prints the seL4 banner,
runs through `vka_alloc_untyped`'s size-probe, then sweeps the test
suite to completion):

```text
ELF-loader started on (HART 0) (NODES 1)
  ...
ELF-loading image 'kernel' to 80200000
  paddr=[80200000..81317fff]
  vaddr=[ffffffff80200000..ffffffff81317fff]
ELF-loading image 'rootserver' to ...
Jumping to kernel-image entry point...

microkernel: Rust kernel booted (S-mode, Sv39)
  ...
  --- transferring control to rootserver ---

seL4 Test
=========

Starting test suite sel4test
Starting test 0:   Test that there are tests
Starting test 1:   SYSCALL0000
...
Starting test 15:  SYSCALL0017
Starting test 16:  BIND0001
Starting test 17:  BIND0002
...
Starting test 114: VSPACE0006
Starting test 116: Test all tests ran
Test suite passed. 116 tests passed. 51 tests disabled.
All is well in the universe
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
│       ├── kernel/
│       │   ├── boot.rs        # bringup_rootserver
│       │   └── bootmem.rs     # bump page allocator
│       ├── object/
│       │   ├── cap.rs         # cap_t bit layouts (Untyped/CNode/Frame/PT/EP/Ntfn/…)
│       │   ├── mdb.rs         # mdb_node_t
│       │   ├── cnode.rs       # Cte + cnode_at / install_initial_cap / mdb_*
│       │   ├── untyped.rs     # free-range splitter, untyped cap factory
│       │   ├── notification.rs # min. Notification (state + badge + signal/wait)
│       │   ├── irq.rs          # min. IRQHandler table + notification binding
│       │   ├── endpoint.rs    # Endpoint (16 B: state-packed head ptr + tail),
│       │   │                  #   wait-list queue ops, finalize wakes waiters
│       │   ├── tcb.rs         # Tcb struct (context + scheduler/IPC state),
│       │   │                  #   256-bin runqueue + bitmap, init on Retype,
│       │   │                  #   finalize on revoke
│       │   └── asid.rs        # 64-entry ASID → root-PT-KVA table
│       └── api/
│           ├── thread.rs      # rootserver thread record (CSpace/VSpace/IPCBuf)
│           ├── cspace.rs      # single-level CSpace lookup (CPtr → Cte*)
│           ├── syscall.rs     # seL4_Call dispatch + error reply encoding +
│           │                  #   Send/Recv slow-path on Notification caps
│           └── invocation.rs  # Untyped_Retype, Page_Map, PageTable_Map, CNode
│                              #   ops, isCapRevocable, finalize_cap(Frame/CNode)
└── tools/
    ├── pack-image.sh      # rebuild Rust kernel + ninja repackage + emit image
    ├── simulate.sh        # qemu wrapper (standalone or packed image)
    └── run-tests.sh       # one-shot CI runner: boots image, watches for
                           #   "Test suite passed", exits 0/1/2
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

# Headless / CI mode — boots the packed image, watches for the
# upstream "Test suite passed." banner, prints a one-line summary, and
# exits 0 on success / 1 on failure / 2 on timeout (default 180 s):
./tools/run-tests.sh           # quiet
./tools/run-tests.sh -v        # stream QEMU output as it runs
TIMEOUT=60 ./tools/run-tests.sh
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

## Next steps (M4)

With the enabled sel4test suite green, the remaining work is about
unlocking the 51 upstream-disabled tests and tightening semantics that
are currently implemented only as far as sel4test needs:

1. **Interrupt and timer stack.** The IRQControl/IRQHandler ABI path now
   exists for cap issuing and notification binding, but the remaining M4
   work is real PLIC claim/complete/masking, trigger programming, and
   userspace timer enablement so the interrupt/preemption groups can be
   turned back on.
2. **Debug exceptions.** Implement hardware breakpoint and debug-fault
   forwarding needed by the `BREAKPOINT_*` tests still pruned from the
   active suite.
3. **Full cap-transfer/generalisation pass.** The current IPC transfer
   path intentionally covers the pre-MCS single receive-slot case. The
   next conformance pass should cover multi-cap edge cases, endpoint
   unwrapping details, and cleanup paths beyond the serial-server use.
4. **Zombie/finalisation fidelity.** CNode/TCB finalisation is good
   enough for the enabled tests, but should be brought closer to the C
   kernel's Zombie reduction model before expanding coverage further.
