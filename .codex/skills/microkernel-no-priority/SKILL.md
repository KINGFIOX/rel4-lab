---
name: microkernel-no-priority
description: Keep this Rust RV64/LoongArch seL4-style microkernel free of priority-based scheduling decisions while preserving seL4-compatible priority APIs for user-space portability. Use when reviewing, planning, maintaining, or changing scheduler, TCB, runqueue, IPC donation, capability invocation, thread-control code, or user-space assumptions so priority values may be set but do not affect dispatch, ordering, donation, inheritance, or correctness unless explicitly requested.
---

# Microkernel No Priority

## Intent

Use this skill to keep scheduling as simple round-robin among runnable threads. User-space may set seL4-style priority and MCP values so the same code can build or run on seL4 and rel4, but rel4 must not choose a thread because of those values.

IPC, reply, notification, and wakeup paths must not donate, inherit, raise, lower, compare, or otherwise interpret priorities for scheduler behavior.

## Avoid

Treat these as out of scope unless the user explicitly requests priority scheduling:

- Per-TCB priority and maximum-controlled-priority fields that influence dispatch.
- Priority-indexed ready queues, ready bitmaps, and highest-priority selection logic.
- Priority donation, priority inheritance, priority boosting, or priority-based IPC ordering.
- Tests or compatibility shims that preserve priority behavior while pretending the scheduler is round-robin.

## Compatibility Policy

- Keep `TCBSetPriority`, `TCBSetMCPriority`, and `TCBSetSchedParams` source-compatible when user-space needs to run on both seL4 and rel4.
- These calls may validate obvious shape/range errors and store metadata if useful for debugging, but they must not affect ready-queue placement, runqueue ordering, IPC delivery order, donation, or wakeup behavior.
- User-space written for this project must not depend on priority values for correctness, progress, timing, IPC ordering, or fairness. If a program needs ordering, make it explicit in IPC/protocol logic rather than relying on priority.
- sel4test adaptations for rel4 must disable tests whose expected result is priority-sensitive scheduler behavior. This includes tests that require higher-priority runnable threads to run first or immediately, priority changes to force rescheduling, priority-ordered IPC delivery, or MCP/priority limits to be meaningful scheduler policy.
- Current sel4test examples in this category include `SCHED0003`, `SCHED0004`, `SCHED0005`, `SCHED0006`, and `SCHED0020`.
- Do not hide ordinary IPC rights, reply, CSpace, VSpace, or fault ABI failures behind this policy just because a test also uses helper priorities for setup. Only disable tests where priority scheduling is the behavior under test.

## Preserve

Keep these behaviors available:

- A single runnable queue per core, or the simplest equivalent structure, with FIFO enqueue and round-robin rotation on explicit yield.
- Blocking and unblocking through IPC, notifications, faults, IRQ delivery, suspend/resume, and explicit syscalls.
- CPU affinity and SMP core selection if needed for multicore correctness.
- Architecture-neutral scheduler interfaces shared by `riscv64` and `loongarch64`.
- seL4 user-space source compatibility for priority-setting APIs, as long as rel4 semantics remain priority-insensitive.

## Workflow

1. Inspect existing diffs before editing with `git status --short` and task-scoped `git diff`.
2. Search for priority terms with `rg -n "priority|prio|MCP|mcp|Priority|setPriority|tcbPriority" kernel userspace tools`.
3. Keep priority policy out of shared scheduler and TCB code before touching architecture trap handlers.
4. Replace priority queues with a round-robin runnable queue. Enqueue newly runnable TCBs at the tail unless an existing non-priority IPC invariant requires a narrower choice.
5. Preserve explicit `Yield` by rotating the current runnable TCB to the tail of the round-robin queue.
6. Keep RISC-V and LoongArch behavior symmetric; priority-insensitive behavior should normally be shared scheduler code, not architecture-specific branches.
7. When changing user-space, avoid assumptions that higher priority makes a task run first or receive IPC first; keep priority calls only for seL4 portability.

## Validation

Use the smallest useful validation stage:

- Rust-only edits: `cargo fmt --all --check`, then `cargo check`.
- Scheduler-sensitive changes: run focused sel4tests for yield, IPC ordering, notifications, and multicore behavior on RISC-V first.
- Architecture parity: run matching LoongArch build/test commands when shared scheduler code or LoongArch trap code changed.
- xv6 impact: run a targeted xv6 program such as `tools/run-xv6-user.py forktest` before broad `usertests`.

Do not claim priority scheduling avoidance is complete until temporary diagnostics are cleaned up, priority APIs are behaviorally no-op on rel4, user-space does not depend on priority semantics, and relevant focused validations pass.
