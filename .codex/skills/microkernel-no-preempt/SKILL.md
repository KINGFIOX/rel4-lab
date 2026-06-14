---
name: microkernel-no-preempt
description: Keep this Rust RV64/LoongArch seL4-style microkernel free of preemptive scheduler behavior while keeping user-space portable across seL4 and rel4. Use when rolling back, reviewing, planning, or changing timer, trap, scheduler, runqueue, context-switch code, or user-space assumptions so timer-driven preemption, timeslice expiry, quantum rotation, asynchronous budget charging, and involuntary scheduler switches are removed or kept out of scope unless the user explicitly asks for preemptive scheduling.
---

# Microkernel No Preempt

## Intent

Use this skill to keep scheduling cooperative/non-preemptive. Threads should switch when they block, yield, fault into the kernel, or make explicit syscalls that change runnable state, not because a timer interrupt expired a quantum.

User-space written for this project must not rely on preemptive scheduling for correctness, progress, ordering, fairness, or timing. It may still run on seL4, where timer preemption exists, but the program logic must also work on rel4 when context switches happen only at explicit kernel interaction points.

## Remove Or Avoid

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

- Prefer user-space source compatibility with seL4, but do not require rel4 to emulate seL4 timer preemption or quantum-expiry behavior.
- User-space may use explicit `Yield`, blocking IPC, notifications, sleeps, or other explicit synchronization points to make progress portable between seL4 and rel4.
- Do not introduce tests, service loops, or user programs that assume a CPU-bound thread will be involuntarily preempted so another runnable thread can run.
- If a user-space workflow needs interleaving, make it explicit in protocol logic rather than relying on timer-driven scheduler interruption.

## Workflow

1. Inspect existing diffs before editing with `git status --short` and task-scoped `git diff`.
2. Remove preemption policy from shared scheduler code before modifying architecture trap handlers.
3. In `kernel/src/arch/riscv64/trap.rs` and `kernel/src/arch/loongarch64/trap.rs`, keep timer handlers focused on interrupt delivery and timer reprogramming, not scheduler quantum expiry.
4. Keep runqueue operations deterministic: enqueue runnable threads, dequeue selected threads, and reschedule only after explicit kernel events.
5. Make RISC-V and LoongArch trap/timer behavior symmetric unless a hardware difference requires a narrow arch-specific branch.
6. When changing user-space, remove assumptions that timer preemption will provide fairness or progress; add explicit yield/blocking/synchronization where needed.

## Validation

Use the smallest useful validation stage:

- Rust-only edits: `cargo fmt --all --check`, then `cargo check`.
- Focused seL4 checks: choose tests around IPC, yield, timers, or interrupts affected by the edit.
- Architecture parity: validate both `ARCH=riscv64` and `ARCH=loongarch64` when shared scheduling or both trap handlers changed.
- xv6 impact: run a targeted xv6 program such as `tools/run-xv6-user.py forktest` before broad `usertests`.

Do not claim preemption removal is complete until temporary diagnostics are removed and relevant focused validations pass.
