---
name: microkernel-no-preempt
description: Keep this Rust RV64/x86_64 seL4-style microkernel free of timeslice, quantum, and budget-accounting scheduler behavior and free of priority-driven preemption, and keep user-space from depending on preemption, while preserving seL4-compatible user-space source portability. Use when reviewing, planning, maintaining, or changing timer, trap, scheduler, runqueue, context-switch code, or project user-space assumptions so timeslice expiry accounting, quantum bookkeeping, asynchronous budget charging, priority-driven preemption, and any reliance on preemption for correctness, progress, ordering, fairness, or timing stay absent unless the user explicitly asks for richer scheduling.
---

# Microkernel No Preempt

## Intent

Use this skill to keep scheduling free of timeslice, quantum, and budget accounting, to keep priority out of scheduler decisions, and to keep user-space from depending on preemption.

User-space written for this project should be portable across seL4 and rel4. It must be written as if preemption is not a correctness or progress guarantee, and equally as if uninterleaved execution is not a guarantee either. Neither outcome is promised.

## Current Behavior

Read this before describing the scheduler. The previous version of this skill got it wrong, and so did the README and milestone docs.

- `kernel_exit` appends the current runnable thread to the tail of its core runqueue and then takes the head. It runs on every trap cause, including timer interrupts. See `kernel_exit` in `kernel/src/arch/riscv64/kernel/trap.rs`. The x86_64 backend is staged (no trap yet).
- Involuntary, timer-driven context switches therefore do happen whenever another runnable thread is queued on the same core. This is the same append-tail-and-reschedule mechanism as upstream non-MCS `timerTick` in `third_party/sel4test/kernel/src/kernel/thread.c`, with an effective `CONFIG_TIME_SLICE` of one tick.
- The only exception is the one-shot resume hint `continue_current_once` in `kernel/src/object/tcb.rs`, set only by the `TCBResume` invocation.
- What is genuinely absent is accounting, not switching: no per-TCB timeslice counter, no consumed-time charging, and no priority-driven preemption.
- Do not describe this kernel as cooperative or non-preemptive. If you need a short label, use "unprioritised round-robin without timeslice accounting".

## Avoid

Treat these as out of scope unless the user explicitly requests richer scheduling:

- Per-TCB timeslice or quantum counters, and reschedule decisions driven by their expiry.
- Charging consumed time to a TCB or scheduling context for scheduler policy.
- Priority-driven preemption: taking the CPU away because another thread has a higher priority.
- Per-hart preferred/resume targets whose purpose is to implement a timeslice policy.
- Widening the existing rotation, for example adding asynchronous switch points beyond the current kernel-exit rotation.

## Preserve

Keep these behaviors available:

- Explicit `Yield` behavior if the ABI exposes it and tests require it.
- Scheduler selection after blocking syscalls, unblocking IPC, thread suspend/resume, faults, and explicit kernel entries.
- Hardware timer interrupt delivery for clock/IRQ functionality when user-visible services or tests need it.
- Idle wakeups and interrupt acknowledgement needed to avoid deadlock.

## Compatibility Policy

- Prefer source compatibility with seL4 user programs: timer APIs, `Yield`, blocking IPC, notifications, sleeps, or related constants may exist if they are needed for the same user binary/source to build or run on seL4 and rel4.
- These compatibility paths may expose time or interrupt services, but they must not add seL4 quantum expiry accounting, consumed-time charging, or priority-driven preemption.
- User-space written for this project must remain correct both when it is switched away at an arbitrary kernel exit and when it runs a long stretch without being switched away. Do not encode either assumption.
- Do not use CPU-bound busy loops, implicit time slicing, scheduler tick side effects, or assumed involuntary interleaving as part of a program's correctness, progress, timing, IPC ordering, or fairness story.
- If a workflow needs another runnable thread to make progress, make that dependency explicit with `Yield`, blocking IPC, notifications, sleeps, or protocol-level synchronization. Treat explicit coordination as the portability boundary between seL4 and rel4.
- Do not introduce tests, service loops, or user programs that assume a CPU-bound thread will be involuntarily preempted so another runnable thread can run.
- sel4test adaptations for rel4 must disable tests whose expected result requires timeslice accounting, budget/timeout-fault semantics, priority-driven preemption, or long-running kernel operations being preempted mid-operation.
- `FPU0001`, `SCHED0021`, and `PREEMPT_REVOKE` were disabled on the assumption that rel4 never preempts. That assumption was wrong, so their status is pending re-validation. Re-run them before citing this policy as the reason they are disabled, and record the real failure reason if they still fail.
- Keep explicitly-synchronised equivalents when they test supported behavior, but do not rewrite a preemption test so broadly that it no longer checks the behavior named by the test.

## Workflow

1. Inspect existing diffs before editing with `git status --short` and task-scoped `git diff`.
2. Keep preemption policy out of shared scheduler code before modifying architecture trap handlers.
3. Keep the timer handlers in `kernel/src/arch/riscv64/kernel/trap.rs` focused on interrupt delivery and timer reprogramming, not quantum accounting. Note that the rotation itself lives in the shared `kernel_exit` path rather than in the timer handler, so review `kernel_exit` when changing switch policy.
4. Keep runqueue operations deterministic: enqueue runnable threads at the tail and dequeue the selected thread. If you change when rescheduling happens, update `README.md` and `docs/milestones/sel4.md` in the same change so the described policy stays true.
5. Keep RISC-V trap/timer behavior as the reference. The x86_64 backend is staged (no trap yet).
6. When changing user-space owned by this project, write it so it remains correct on both seL4 and rel4 without relying on timer preemption; add explicit yield/blocking/synchronization where interleaving is required.

## Validation

Use the smallest useful validation stage:

- Rust-only edits: `cargo fmt --all --check`, then `cargo check`.
- Focused seL4 checks: choose tests around IPC, yield, timers, or interrupts affected by the edit.
- Architecture parity: validate `ARCH=riscv64` when shared scheduling or the trap handler changed. The x86_64 backend is staged (no trap yet).
- xv6 impact: run a targeted xv6 program such as `tools/run-xv6-user.py forktest` before broad `usertests`.

Do not claim the scheduler is non-preemptive or cooperative; verify the `kernel_exit` rotation before making any statement about switch policy. Do not claim timeslice/budget avoidance is complete until temporary diagnostics are cleaned up and relevant focused validations pass.
