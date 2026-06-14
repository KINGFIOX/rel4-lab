# RISC-V FPU seL4 Alignment Matrix

This note is the requirement-by-requirement checklist for aligning the Rust
RV64 FPU implementation with the vendored upstream seL4 baseline under
`third_party/sel4-lab/sel4test/kernel`. It covers the current
`riscv64gc-unknown-none-elf`, `KernelHaveFPU`, two-hart-capable build for the
single-domain rel4 scheduler. It does not claim a formal proof and does not
expand scope beyond seL4's RISC-V FPU semantics.

## Status

The implementation is source-audited as seL4-aligned for the active
single-domain rel4 build. The local access shadow now follows upstream RISC-V seL4's
`enableFpu()` / `disableFpu()` split: access toggles update only the per-core
shadow state, while explicit boot/trap boundaries clear supervisor `sstatus.FS`
before ordinary Rust dispatch and the user restore path writes the selected
TCB's saved `sstatus`.

The current single-domain kernel intentionally has no live domain-rotation path
and no MCS scheduling-context handoff. If multi-domain scheduling or MCS-style
scheduling contexts are reintroduced, the upstream `prepareSetDomain` /
`prepareNextDomain` and related FPU-owner release points must be implemented
before this matrix can remain green.

## Evidence Gates

The following gates are the minimum evidence before treating FPU alignment as
closed for a given commit:

```sh
python3 -m py_compile tools/audit-kernel-fpu.py tools/audit-fpu-lifecycle.py
./tools/audit-fpu-lifecycle.py --verbose
./tools/audit-kernel-fpu.py --build --verbose
SEL4TEST_REGEX='FPU000[0-4]' ./tools/pack-image.py
./tools/run-tests.py
```

Broader syscall/fault/register slices are still useful because FPU-disabled
faults share the `UserException` and TCB register ABI, but the commands above
are the focused FPU closure gate.

Latest focused evidence, from 2026-06-12 after the shadow-only access-toggle
alignment:

```text
python3 -m py_compile tools/audit-kernel-fpu.py tools/audit-fpu-lifecycle.py
  passed
./tools/audit-fpu-lifecycle.py --verbose
  PASS: 114 seL4 FPU lifecycle source checks passed
./tools/audit-kernel-fpu.py --build --verbose
  PASS: 464 FP-register/fcsr instructions confined to kernel/src/arch/riscv64/fpu.rs
SEL4TEST_REGEX='FPU000[0-4]' ./tools/pack-image.py
  image ready
./tools/run-tests.py
  PASS: Test suite passed. 6 tests passed. 1 tests disabled.
```

## Requirement Matrix

| Requirement | Upstream seL4 anchor | Local evidence | Audit/runtime evidence | Status |
|-------------|----------------------|----------------|------------------------|--------|
| Build enables the RISC-V F/D extension and `KernelHaveFPU`. | `src/arch/riscv/config.cmake` | `.cargo/config.toml`, `rust-toolchain.toml`, `tools/pack-image.py` | `audit-fpu-lifecycle.py`: target, packer, and D/F checks | Aligned |
| Kernel and sel4test use the same double-precision FPU context layout. | `include/arch/riscv/arch/machine/registerset.h` | `kernel/src/arch/riscv64/trap.rs` | Layout, size, and offset checks for `UserContext` and `FpuState` | Aligned |
| Boot starts each core with no current FPU owner and disabled FPU access shadow. | `src/kernel/boot.c`, `src/arch/riscv/kernel/boot.c` | `kernel/src/arch/riscv64/fpu.rs`, `kernel/src/arch/riscv64/boot.rs`, `kernel/src/kernel/boot.rs` | Owner-zero, access-shadow, primary, and secondary init checks | Aligned |
| Boot initializes hardware FPU state with `FS=Clean`, `fcsr=0`, then disables access. | `init_fpu()` in `src/arch/riscv/kernel/boot.c` | `fpu::init_current_core()` | `per-core FPU init matches seL4 reset shape` | Aligned |
| Ordinary TCBs start with `seL4_TCBFlag_NoFlag`, not `fpuDisabled`. | `createObject()` and `Arch_initContext()` | `tcb::init()`, `Tcb::zero()` | Ordinary TCB zero/init checks | Aligned |
| The boot rootserver TCB starts from an explicit zero FPU image before first user return. | `create_initial_thread()` | `ROOTSERVER_TCB`, `UserContext::zero()`, `FpuState::zero()` | Static rootserver and first-return checks | Aligned |
| Saved TCB `sstatus.FS` is cleared before optionally setting `FS=Clean`. | `set_tcb_fs_state()` | `tcb::set_fpu_context_enabled()` | Upstream/local FS-state helper checks | Aligned |
| Save/load covers all `f0..f31` registers and `fcsr`. | `saveFpuState()`, `loadFpuState()` | `save_fpu_state()`, `load_fpu_state()` | Full register-pattern and `fcsr` checks | Aligned |
| Save/load inline asm remains memory-visible to the Rust compiler. | seL4 C inline asm touches the user FPU state object | Rust `asm!` omits `nomem` on save/load blocks | `local FPU save/load asm keeps FPU state memory visible` | Aligned |
| Per-core lazy ownership saves the old native owner before loading the new owner. | `switchLocalFpuOwner()` | `switch_local_owner()` | Owner-switch ordering check | Aligned |
| Lazy restore disables access for `fpuDisabled`, reuses a native owner, or switches owner. | `lazyFPURestore()` | `fpu::lazy_restore()` | Disabled/native/switch checks | Aligned |
| `TCB_SetFlags` uses two clear/set words, masks to seL4 flags, and returns one flags word for Calls only. | `decodeSetFlags()`, `invokeSetFlags()`, libsel4 object API XML | `handle_thread_inner()`, `success_reply_length()`, `sel4-user` helper | Decode, reply-length, send-only, and XML-contract checks | Aligned |
| Setting `fpuDisabled` releases any live owner and clears the saved TCB FS state. | `invokeSetFlags()` | `tcb::set_flags()` | SetFlags side-effect check | Aligned |
| Re-enabling the current TCB refreshes lazy FPU state before returning to user mode. | `invokeSetFlags()` plus `lazyFPURestore()` | `tcb::set_flags()` | SetFlags current-thread restore check | Aligned |
| FPU-disabled execution faults as RISC-V illegal instruction `UserException(Number=2, Code=0)`. | `c_traps.c`, `handleUserLevelFault()` | `trap.rs::fault_message()` | UserException fault-shape checks | Aligned |
| UserException replies restore only FaultIP and SP, not Number or Code. | `EXCEPTION_MESSAGE`, `copyMRsFaultReply()` | `api/ipc.rs::apply_user_exception_reply()` | Reply writeback check | Aligned |
| TCB register and CopyRegisters ABI excludes FPU state on RISC-V. | `frameRegisters[]`, `gpRegisters[]`, `Arch_decodeTransfer()` | centralized register arrays and CopyRegisters path | Register ABI and CopyRegisters no-FPU checks | Aligned |
| Final Thread-cap deletion releases a live FPU owner before TCB storage reuse. | `finaliseCap(Thread)`, `Arch_prepareThreadDelete()` | `tcb::finalize()` | Finalisation release check | Aligned |
| Affinity migration releases the old-core FPU owner before updating affinity. | `migrateTCB()` | `tcb::set_affinity()` | Affinity release check | Aligned |
| Remote owner release uses the upstream remote FPU-owner operation and does not deschedule the target TCB. | `IpiRemoteCall_switchFpuOwner` | `smp::remote_fpu_owner_release()` | Remote release/no-deschedule checks | Aligned |
| Ordinary remote TCB stall remains separate from remote FPU owner release. | `IpiRemoteCall_Stall` vs `IpiRemoteCall_switchFpuOwner` | `kernel/src/kernel/smp.rs` | Remote-stall forbidden-pattern checks | Aligned |
| Idle handoff does not synthesize an idle-thread FPU release. | `configureIdleThread()`, `switchToIdleThread()` | idle scheduler and `clear_current_state()` | Idle handoff checks | Aligned |
| Every normal user return refreshes FPU state before writing saved `sstatus` and `sret`. | `restore_user_context()` | `prepare_for_user_restore()`, trap restore asm | Restore-boundary and trap-asm checks | Aligned |
| No local IPC fastpath bypasses FPU restore policy. | RISC-V `fastpath_restore()` and signal slowpath guard | absence of local fastpath module | Fastpath absence check | Aligned for current kernel |
| Future local fastpath must slowpath when the destination is a native FPU owner. | `fastpath_signal()` guarded by `nativeThreadUsingFPU(dest)` | no current local fastpath | Upstream anchor plus local absence check | Future guard |
| Domain handoff releases FPU owners when changing away from a live domain. | `prepareSetDomain()`, `prepareNextDomain()` | current `NUM_DOMAINS = 1` single-domain build, single-domain `DomainSet`, and scheduler without live domain rotation | Upstream anchor plus local single-domain checks | Aligned for current kernel; pending if multi-domain is enabled |
| Release ELF confines all emitted FP-register and FPU CSR instructions to FPU helpers. | seL4 keeps FPU instructions in machine FPU helpers | `kernel/src/arch/riscv64/fpu.rs` | `audit-kernel-fpu.py --build --verbose` | Passed 2026-06-12 |
| Focused sel4test FPU behavior passes. | `projects/sel4test/.../tests/fpu.c` | packed Rust kernel sel4test image | `FPU000[0-4]` pack/run gate | Passed 2026-06-12 |

## Remaining Closure Work

Before marking the persistent FPU alignment goal complete after future kernel
changes, rerun the evidence gates on the current commit and record their output
in `sel4.md` or this file. If the focused FPU sel4test slice or release
instruction audit fails, treat the failed row in the matrix as incomplete and
fix that behavior rather than weakening the requirement.
