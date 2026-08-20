---
name: microkernel-no-preempt
description: Keep this Rust RV64/x86_64 seL4-style microkernel on unprioritised timeslice round-robin without priority-driven preemption, and keep user-space from depending on mid-slice involuntary switches, while preserving seL4-compatible user-space source portability. Use when reviewing, planning, maintaining, or changing timer, trap, scheduler, runqueue, context-switch code, or project user-space assumptions so a still-runnable thread keeps the CPU until its timeslice expires or it Yields/blocks/is suspended, and so priority-driven preemption, and any reliance on mid-slice preemption for correctness stay absent unless the user explicitly asks for richer scheduling.
---

# Microkernel No Preempt

## Intent

Use this skill to keep scheduling as unprioritised timeslice round-robin, to keep priority out of scheduler decisions, and to keep user-space from depending on being switched away before a timeslice expires.

In this repository, "no preemption" means: **if the current thread is still runnable and its timeslice has not expired, do not schedule another thread**. Yield, blocking, and suspend still switch. Timer expiry after `TIME_SLICE_TICKS` ticks still rotates.

User-space written for this project should be portable across seL4 and rel4. It must be written as if mid-slice involuntary switching is not a correctness or progress guarantee.

## Current Behavior

- Each TCB has a `time_slice` initialised to `TIME_SLICE_TICKS` (5). `tcb::timer_tick` in `kernel/src/object/tcb.rs` matches upstream `timerTick` in `third_party/sel4test/kernel/src/kernel/thread.c`: decrement while greater than one; on zero, refill and set `reschedule_required`.
- `kernel_exit` in `kernel/src/arch/riscv64/kernel/trap.rs` and `kernel/src/arch/x86_64/kernel/trap.rs` resumes the current runnable thread unless `continue_current_once` (from `TCBResume`) or `take_reschedule_required()` (from timeslice expiry or `Yield`) says otherwise.
- `Yield` calls `tcb::yield_current`: enqueue at the tail and request a rotation.
- Blocking IPC, suspend, and an empty runqueue still pick another thread or idle. A newly woken thread is appended and does not steal the CPU.
- There is no priority-driven preemption.

## Avoid

Treat these as out of scope unless the user explicitly requests richer scheduling:

- Rotating a still-runnable thread on an ordinary trap, syscall, or IRQ while its timeslice remains.
- Priority-driven preemption: taking the CPU away because another thread has a higher priority.
- timeout-fault budget enforcement, or sched-context refill policy.
- Widening rotation beyond timeslice expiry, Yield, and not-runnable/idle paths.

## Preserve

Keep these behaviors available:

- Per-TCB timeslice accounting driven only by the timer path (`handle_timer_interrupt` / `service_due_timer_interrupts`).
- Explicit `Yield` rotation.
- Scheduler selection after blocking syscalls, unblocking IPC, thread suspend/resume, faults, and explicit kernel entries when current is no longer runnable.
- Hardware timer interrupt delivery for clock/IRQ functionality when user-visible services or tests need it.
- Idle wakeups and interrupt acknowledgement needed to avoid deadlock.

## Compatibility Policy

- Prefer source compatibility with seL4 user programs: timer APIs, `Yield`, blocking IPC, notifications, sleeps, or related constants may exist if they are needed for the same user binary/source to build or run on seL4 and rel4.
- These compatibility paths may expose time or interrupt services, but they must not add priority-driven preemption.
- User-space written for this project must remain correct when a runnable thread runs until its slice expires. Do not assume an arbitrary kernel exit will switch it away.
- If a workflow needs another runnable thread to make progress before the current slice ends, make that dependency explicit with `Yield`, blocking IPC, notifications, sleeps, or protocol-level synchronization.
- Do not introduce tests, service loops, or user programs that assume a CPU-bound thread will be involuntarily switched away on every trap.
- sel4test adaptations for rel4 must disable tests whose expected result is priority-driven preemption or long-running kernel operations being preempted mid-operation. Timeslice-expiry rotation is supported; do not disable a test solely because a timer can rotate after `TIME_SLICE_TICKS` ticks.
- Re-validated 2026-08-20: `REL4_HAS_TIMER_PREEMPTION` is 1. `FPU0001` and `SCHED0021` passed on RV64 unicore once compiled in. `SCHED0021` stays behind upstream `!CONFIG_SIMULATION` (qemu-virt forces `SIMULATION=ON`). `PREEMPT_REVOKE` stays behind `REL4_HAS_PRIORITY_SCHEDULING`: on x86 it grew cap tables until OOM because a still-runnable revoke thread keeps the CPU and priority does not steal it.

## Workflow

1. Inspect existing diffs before editing with `git status --short` and task-scoped `git diff`.
2. Keep timeslice policy in shared scheduler code (`kernel/src/object/tcb.rs`) before modifying architecture trap handlers.
3. Timer handlers in `kernel/src/arch/riscv64/kernel/trap.rs` and `kernel/src/arch/x86_64/kernel/trap.rs` must call `tcb::timer_tick` and reprogram the timer. They must not rotate the runqueue themselves.
4. Keep runqueue operations deterministic: enqueue runnable threads at the tail and dequeue the selected thread. If you change when rescheduling happens, update `README.md` and `docs/kernel.md` in the same change so the described policy stays true.
5. Keep RISC-V and x86_64 behavior symmetric. Both backends already run `kernel_exit` on trap return.
6. When changing user-space owned by this project, write it so it remains correct on both seL4 and rel4 without relying on mid-slice involuntary switching.

## Validation

Use the smallest useful validation stage:

- Rust-only edits: `cargo fmt --all --check`, then `cargo check`.
- Focused seL4 checks: choose tests around IPC, yield, timers, or interrupts affected by the edit.
- Architecture parity: validate `ARCH=riscv64` when shared scheduling or the RISC-V trap handler changed. When the x86 trap, timer, or shared `kernel_exit` policy changed, also run `TIMEOUT=60 ARCH=x86_64 SMP=OFF NUM_NODES=1 tools/run-hello.py`.
- linux-compat impact: run `TIMEOUT=180 ARCH=riscv64 tools/run-ltp.py`.

Do not describe this kernel as rotating on every trap. Verify `timer_tick` and `take_reschedule_required` before making any statement about switch policy.
