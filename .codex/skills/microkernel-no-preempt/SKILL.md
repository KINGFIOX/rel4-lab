---
name: microkernel-no-preempt
description: Keep this Rust RV64/LoongArch seL4-style microkernel free of preemptive scheduler behavior while preserving seL4-compatible user-space source portability. Use when reviewing, planning, maintaining, or changing timer, trap, scheduler, runqueue, context-switch code, or project user-space assumptions so timer-driven preemption, timeslice expiry, quantum rotation, asynchronous budget charging, involuntary scheduler switches, and any reliance on preemption for correctness, progress, ordering, fairness, or timing stay absent unless the user explicitly asks for preemptive scheduling.
---

# Microkernel No Preempt

## Intent

Use this skill to keep scheduling cooperative/non-preemptive. Threads should switch when they block, yield, fault into the kernel, or make explicit syscalls that change runnable state, not because a timer interrupt expired a quantum.

User-space written for this project should be portable across seL4 and rel4. It may run on seL4, where timer preemption exists, but it must be written as if preemption is not a correctness or progress guarantee. The same source should also run on rel4, where context switches happen only at explicit kernel interaction points.

Preemption-related effects must not be part of the rel4 user-space contract. Project user programs may tolerate being preempted on seL4, but they must not rely on timer preemption for correctness, progress, ordering, fairness, or timing on either kernel.

## Avoid

Treat these as out of scope unless the user explicitly requests preemptive scheduling:

- Timer-driven timeslice expiry and round-robin quantum rotation.
- Charging the current TCB on every timer interrupt for scheduler policy.
- Involuntary context switches caused only by asynchronous timer interrupts.
- Per-hart preferred/resume targets whose purpose is to compensate for timer preemption policy.

## Preserve

Keep these behaviors available:

- Explicit `Yield` behavior if the ABI exposes it and tests require it.
- Scheduler selection after blocking syscalls, unblocking IPC, thread suspend/resume, faults, and explicit kernel entries.
- Hardware timer interrupt delivery for clock/IRQ functionality when user-visible services or tests need it.
- Idle wakeups and interrupt acknowledgement needed to avoid deadlock.

## Compatibility Policy

- Prefer source compatibility with seL4 user programs: timer APIs, `Yield`, blocking IPC, notifications, sleeps, or related constants may exist if they are needed for the same user binary/source to build or run on seL4 and rel4.
- These compatibility paths may expose time or interrupt services, but they must not make rel4 emulate seL4 quantum expiry, timer-driven ready-queue rotation, or involuntary preemption.
- User-space written for this project may set up workloads that are preemptible on seL4, but it must remain correct when rel4 never preempts a running thread.
- Do not use CPU-bound busy loops, implicit time slicing, scheduler tick side effects, or assumed involuntary interleaving as part of a program's correctness, progress, timing, IPC ordering, or fairness story.
- If a workflow needs another runnable thread to make progress, make that dependency explicit with `Yield`, blocking IPC, notifications, sleeps, or protocol-level synchronization. Treat explicit coordination as the portability boundary between seL4 and rel4.
- Do not introduce tests, service loops, or user programs that assume a CPU-bound thread will be involuntarily preempted so another runnable thread can run.

## Workflow

1. Inspect existing diffs before editing with `git status --short` and task-scoped `git diff`.
2. Keep preemption policy out of shared scheduler code before modifying architecture trap handlers.
3. In `kernel/src/arch/riscv64/trap.rs` and `kernel/src/arch/loongarch64/trap.rs`, keep timer handlers focused on interrupt delivery and timer reprogramming, not scheduler quantum expiry.
4. Keep runqueue operations deterministic: enqueue runnable threads, dequeue selected threads, and reschedule only after explicit kernel events.
5. Make RISC-V and LoongArch trap/timer behavior symmetric unless a hardware difference requires a narrow arch-specific branch.
6. When changing user-space owned by this project, write it so it remains correct on both seL4 and rel4 without relying on timer preemption; add explicit yield/blocking/synchronization where interleaving is required.

## Validation

Use the smallest useful validation stage:

- Rust-only edits: `cargo fmt --all --check`, then `cargo check`.
- Focused seL4 checks: choose tests around IPC, yield, timers, or interrupts affected by the edit.
- Architecture parity: validate both `ARCH=riscv64` and `ARCH=loongarch64` when shared scheduling or both trap handlers changed.
- xv6 impact: run a targeted xv6 program such as `tools/run-xv6-user.py forktest` before broad `usertests`.

Do not claim preemption avoidance is complete until temporary diagnostics are cleaned up and relevant focused validations pass.
