---
name: microkernel-no-preempt
description: Keep this Rust RV64/LoongArch seL4-style microkernel free of preemptive scheduler behavior. Use when rolling back, reviewing, planning, or changing timer, trap, scheduler, runqueue, or context-switch code so timer-driven preemption, timeslice expiry, quantum rotation, asynchronous budget charging, and involuntary scheduler switches are removed or kept out of scope unless the user explicitly asks for preemptive scheduling.
---

# Microkernel No Preempt

## Intent

Use this skill to keep scheduling cooperative/non-preemptive. Threads should switch when they block, yield, fault into the kernel, or make explicit syscalls that change runnable state, not because a timer interrupt expired a quantum.

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

## Workflow

1. Inspect existing diffs before editing with `git status --short` and task-scoped `git diff`.
2. Remove preemption policy from shared scheduler code before modifying architecture trap handlers.
3. In `kernel/src/arch/riscv64/trap.rs` and `kernel/src/arch/loongarch64/trap.rs`, keep timer handlers focused on interrupt delivery and timer reprogramming, not scheduler quantum expiry.
4. Keep runqueue operations deterministic: enqueue runnable threads, dequeue selected threads, and reschedule only after explicit kernel events.
5. Make RISC-V and LoongArch trap/timer behavior symmetric unless a hardware difference requires a narrow arch-specific branch.

## Validation

Use the smallest useful validation stage:

- Rust-only edits: `cargo fmt --all --check`, then `cargo check`.
- Focused seL4 checks: choose tests around IPC, yield, timers, or interrupts affected by the edit.
- Architecture parity: validate both `ARCH=riscv64` and `ARCH=loongarch64` when shared scheduling or both trap handlers changed.
- xv6 impact: run a targeted xv6 program such as `tools/run-xv6-user.py forktest` before broad `usertests`.

Do not claim preemption removal is complete until temporary diagnostics are removed and relevant focused validations pass.
