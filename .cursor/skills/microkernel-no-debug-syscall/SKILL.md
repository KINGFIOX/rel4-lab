---
name: microkernel-no-debug-syscall
description: This kernel does not support seL4 debug syscalls. Use when reviewing or changing trap dispatch, syscall numbers, ABI audits, first-party user-space, or docs so DumpScheduler, Snapshot, SendIPI, DebugRun, and benchmark syscalls stay unimplemented and are not treated as gaps to fill.
---

# Microkernel Has No Debug Syscalls

This kernel does not support seL4 debug syscalls. Do not implement them,
fill the current no-ops, or describe them as unfinished seL4 alignment.

## Do not add

- Real `DebugDumpScheduler` / `DebugSnapshot` / capDL dumps.
- A `DebugSendIpi` that sends IPIs. Keep the existing halt.
- `DebugRun`, benchmark syscalls (`-16`…`-25`), `X86DangerousWRMSR` /
  `RDMSR`, or `VMEnter`.
- First-party callers that need dump, snapshot, or SendIPI for correctness.
- Comments that these handlers are "TODO", "not yet", or a seL4 gap.

Vendored `third_party/sel4test` may still call debug syscalls. That is not
a reason to implement them here.

## Existing numbers

Keep `-9`…`-15` in `kernel/src/abi/syscall.rs` so `SetTLSBase` stays `-29`.
Do not add `-16`…`-28`.

- `DebugDumpScheduler` and `DebugSnapshot` stay empty success.
- `DebugSendIpi` stays halt.
- `DebugPutChar` and `DebugHalt` may keep their current thin handlers
  (console byte, halt). Do not grow them into a debug toolkit.
- `DebugCapIdentify` and `DebugNameThread` may keep their current
  handlers. Do not add more debug operations beside them.

Unrecognized numbers, including the unused debug/benchmark/VTX slots,
remain unknown-syscall faults.

## Preserve

- Ordinary `Call` / `Send` / `Recv` / `Reply` / `Yield` / `SetTLSBase`.
- Kernel and userspace runtime diagnostics through Rust `log` macros, not
  snapshot or scheduler-dump syscalls.

## Validation

- Rust-only edits: `cargo fmt --all --check`, then `cargo check`.
- ABI number edits: `tools/audit-syscall-abi.py` if that script covers the
  changed constants.
