---
name: microkernel-no-mcs
description: This kernel has no MCS surface. Use when reviewing or changing scheduler, TCB, reply, IPC, fault, ABI, or tool code so sched-context, timeout-fault, budget, and refill names stay out of first-party sources.
---

# Microkernel Has No MCS

This kernel does not implement seL4 MCS. Do not add MCS names, constants,
comments, stubs, or "non-MCS" contrasts to first-party sources. Describe
the existing syscall, reply-cap, and fault ABI as seL4, not as a subset
defined against MCS.

## Do not add

- `SchedContext` / `SchedControl` objects, timeout-fault labels, refill or
  budget constants, or comments that mention MCS / `CONFIG_KERNEL_MCS`.
- Compatibility shims that exist only to reject MCS invocations.

Vendored `third_party/sel4test` may still mention MCS. `tools/pack-image.py`
pins that tree's CMake `MCS=OFF` so user-space is built against the same
syscall ABI this kernel implements. Do not honor a `MCS` environment override.

## Preserve

- Ordinary TCB runnable/blocked/restart transitions.
- Endpoint, notification, reply-cap, CSpace, VSpace, IRQ, and user fault
  behavior needed by sel4tests.
- Architecture-neutral scheduler interfaces for `riscv64` and `x86_64`.

## Validation

- Rust-only edits: `cargo fmt --all --check`, then `cargo check`.
- Shared ABI edits: `tools/audit-syscall-abi.py` if that script covers the
  changed constants.
