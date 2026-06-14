---
name: microkernel-no-domain
description: Keep this Rust RV64/LoongArch seL4-style microkernel free of domain scheduling semantics. Use when rolling back, reviewing, planning, or changing scheduler, TCB, timer, runqueue, boot, or configuration code so seL4 domain IDs, domain queues, domain time slices, domain rotation, and domain-aware scheduling decisions are removed or kept out of scope unless the user explicitly asks for domain scheduling.
---

# Microkernel No Domain

## Intent

Use this skill to keep the kernel on a single scheduling domain. The scheduler should choose runnable threads by the kernel's basic priority/queue policy without domain IDs, domain time budgets, or domain rotation.

## Remove Or Avoid

Treat these as out of scope unless the user explicitly requests domain scheduling:

- Per-TCB domain fields that influence scheduler eligibility.
- Per-domain ready queues, domain masks, domain schedules, and domain time accounting.
- Domain timer ticks, domain rotation, and reschedule decisions caused only by domain budget expiry.
- Boot-time or configuration plumbing whose only purpose is to assign or rotate scheduling domains.

## Preserve

Keep these behaviors available:

- CPU affinity and SMP core selection when needed for multicore correctness.
- Basic priority ordering and ready-queue behavior within the single effective scheduling domain.
- Explicit thread suspend/resume, yield, blocking, unblocking, IPC, IRQ, and fault-driven scheduling events.
- Architecture-neutral scheduler interfaces shared by `riscv64` and `loongarch64`.

## Workflow

1. Inspect existing diffs before editing with `git status --short` and task-scoped `git diff`.
2. Search for domain terms with `rg -n "domain|Domain|ksCurDomain|DomainTime|domain_time|tcbDomain" kernel userspace tools`.
3. Remove domain policy from shared scheduler and TCB code before touching architecture trap handlers.
4. Replace domain eligibility checks with single-domain behavior: a runnable thread on the current core is schedulable without comparing a domain ID.
5. Keep RISC-V and LoongArch behavior symmetric; domain removal should normally be shared scheduler code, not architecture-specific branches.

## Validation

Use the smallest useful validation stage:

- Rust-only edits: `cargo fmt --all --check`, then `cargo check`.
- Scheduler-sensitive changes: run focused sel4tests for scheduling, IPC, and multicore behavior on RISC-V first.
- Architecture parity: run matching LoongArch build/test commands when shared scheduler code or LoongArch trap code changed.
- xv6 impact: run a targeted xv6 program such as `tools/run-xv6-user.py forktest` before broad `usertests`.

Do not claim domain scheduling removal is complete until temporary diagnostics are removed and relevant focused validations pass.
