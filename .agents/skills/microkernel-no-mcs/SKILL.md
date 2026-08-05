---
name: microkernel-no-mcs
description: Keep this Rust RV64/LoongArch seL4-style microkernel free of MCS real-time scheduling semantics. Use when reviewing, planning, or changing sched-context, sched-control, timeout-fault, budget accounting, budget donation, reply-consumed-time, refill queue, or MCS-specific IPC/scheduler behavior so those features stay absent or kept out of scope unless the user explicitly asks for real-time OS support.
---

# Microkernel No MCS

## Intent

Use this skill to keep the kernel away from seL4 MCS real-time scheduling. The kernel should keep ordinary seL4-style IPC, capabilities, faults, and runnable/blocked TCB behavior, but not sched-context budget policy or timeout-fault budget enforcement.

## Avoid

Treat these as out of scope unless the user explicitly requests real-time OS support:

- Sched-context capabilities as runtime budget objects, sched-control invocations, sporadic-server refill queues, and release queues.
- Budget charging, consumed-time accounting, reply-consumed-time reporting, and timeout-fault delivery for budget exhaustion.
- Budget donation through reply objects or call stacks.
- MCS-only scheduler-action logic that exists to honor budget release, replenishment, or timeout handling.

## Preserve

Keep these semantics intact while avoiding MCS:

- Basic TCB runnable, blocked-on-send, blocked-on-receive, blocked-on-reply, and restart transitions.
- Endpoint, notification, ordinary reply, CSpace, VSpace, IRQ, and non-timeout user fault behavior needed by sel4tests.
- Architecture-neutral scheduler interfaces usable by both `riscv64` and `loongarch64`.
- xv6 userspace compatibility paths that depend on ordinary IPC and IRQ delivery.

## Workflow

1. Inspect existing diffs before editing with `git status --short` and task-scoped `git diff`.
2. Keep MCS policy out of shared modules, especially `kernel/src/object/reply.rs`, `kernel/src/object/tcb.rs`, and `kernel/src/kernel/smp.rs`; do not recreate `kernel/src/object/sched_context.rs` unless real-time support is explicitly requested.
3. Replace MCS-dependent checks with simpler runnable/blocked checks only when needed for existing IPC correctness.
4. Keep timeout faults for budget exhaustion absent; do not emulate them with compatibility shims.
5. Apply matching architecture changes in `kernel/src/arch/riscv64/` and `kernel/src/arch/loongarch64/` when trap code references MCS behavior.

## Validation

Use the smallest useful validation stage:

- Rust-only edits: `cargo fmt --all --check`, then `cargo check`.
- Focused seL4 checks: `SEL4TEST_REGEX='<test>' ARCH=riscv64 tools/pack-image.py`, then `ARCH=riscv64 tools/run-tests.py`.
- LoongArch parity: run the matching `ARCH=loongarch64` build/test command when shared scheduler or LoongArch trap code changed.
- xv6 impact: run a targeted xv6 program such as `tools/run-xv6-user.py forktest` before broad `usertests`.

Do not claim MCS avoidance is complete until diagnostics are cleaned up and the relevant focused validations pass.
