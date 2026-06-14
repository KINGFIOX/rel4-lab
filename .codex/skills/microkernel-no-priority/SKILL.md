---
name: microkernel-no-priority
description: Keep this Rust RV64/LoongArch seL4-style microkernel free of priority scheduling semantics. Use when rolling back, reviewing, planning, or changing scheduler, TCB, runqueue, IPC donation, capability invocation, or thread-control code so priority fields, priority queues, priority donation/inheritance, and priority-based dispatch are removed or kept out of scope; prefer a single round-robin runnable policy unless the user explicitly asks for priority scheduling.
---

# Microkernel No Priority

## Intent

Use this skill to keep scheduling as simple round-robin among runnable threads. The kernel should not choose a thread because of a priority value, and IPC or reply paths should not donate, inherit, raise, lower, or compare priorities.

## Remove Or Avoid

Treat these as out of scope unless the user explicitly requests priority scheduling:

- Per-TCB priority and maximum-controlled-priority fields that influence dispatch.
- Priority-indexed ready queues, ready bitmaps, and highest-priority selection logic.
- Thread-control operations whose only purpose is setting priority or MCP.
- Priority donation, priority inheritance, priority boosting, or priority-based IPC ordering.
- Tests or compatibility shims that preserve priority behavior while pretending the scheduler is round-robin.

## Preserve

Keep these behaviors available:

- A single runnable queue per core, or the simplest equivalent structure, with FIFO enqueue and round-robin rotation on explicit yield.
- Blocking and unblocking through IPC, notifications, faults, IRQ delivery, suspend/resume, and explicit syscalls.
- CPU affinity and SMP core selection if needed for multicore correctness.
- Architecture-neutral scheduler interfaces shared by `riscv64` and `loongarch64`.

## Workflow

1. Inspect existing diffs before editing with `git status --short` and task-scoped `git diff`.
2. Search for priority terms with `rg -n "priority|prio|MCP|mcp|Priority|setPriority|tcbPriority" kernel userspace tools`.
3. Remove priority policy from shared scheduler and TCB code before touching architecture trap handlers.
4. Replace priority queues with a round-robin runnable queue. Enqueue newly runnable TCBs at the tail unless an existing non-priority IPC invariant requires a narrower choice.
5. Preserve explicit `Yield` by rotating the current runnable TCB to the tail of the round-robin queue.
6. Keep RISC-V and LoongArch behavior symmetric; priority removal should normally be shared scheduler code, not architecture-specific branches.

## Validation

Use the smallest useful validation stage:

- Rust-only edits: `cargo fmt --all --check`, then `cargo check`.
- Scheduler-sensitive changes: run focused sel4tests for yield, IPC ordering, notifications, and multicore behavior on RISC-V first.
- Architecture parity: run matching LoongArch build/test commands when shared scheduler code or LoongArch trap code changed.
- xv6 impact: run a targeted xv6 program such as `tools/run-xv6-user.py forktest` before broad `usertests`.

Do not claim priority scheduling removal is complete until temporary diagnostics are removed and relevant focused validations pass.
