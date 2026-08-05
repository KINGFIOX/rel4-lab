---
name: microkernel-no-domain
description: Keep this Rust RV64/LoongArch seL4-style microkernel on one global scheduling domain. Use when reviewing, planning, maintaining, or changing scheduler, TCB, timer, runqueue, boot, ABI, or configuration code so user-space may remain seL4-compatible but the kernel treats all threads as belonging to one large domain with no domain queues, time slices, rotation, or domain-aware dispatch unless explicitly requested.
---

# Microkernel No Domain

## Intent

Use this skill to keep the kernel on a single global scheduling domain. User-space may see or pass seL4-style domain values for source compatibility, including values that would describe multiple domains on seL4, but rel4 must collapse them all into one effective domain.

Domain values must not affect dispatch, eligibility, IPC ordering, affinity, wakeups, timer behavior, or any other scheduler decision.

## Avoid

Treat these as out of scope unless the user explicitly requests multi-domain scheduling:

- Per-TCB domain fields that influence scheduler eligibility or ordering.
- Per-domain ready queues, domain masks, domain schedules, and domain time accounting.
- Domain timer ticks, domain rotation, and reschedule decisions caused only by domain budget expiry.
- Boot-time or configuration plumbing whose purpose is to assign, rotate, or budget multiple scheduling domains.

## Compatibility Policy

- Prefer source compatibility with seL4 user programs: domain caps, `DomainSet`, BootInfo fields, or constants may exist if they are needed for the same user binary/source to build or run on seL4 and rel4.
- Any accepted domain value or multi-domain configuration is metadata only. Clamp, ignore, or normalize it to the single global domain; never branch scheduling behavior on it.
- User-space written for this project must not rely on multiple domains, domain IDs, domain time, domain rotation, or domain-specific placement for correctness.

## Preserve

Keep these behaviors available:

- CPU affinity and SMP core selection when needed for multicore correctness.
- Basic runnable queue behavior within the single effective scheduling domain.
- Explicit thread suspend/resume, yield, blocking, unblocking, IPC, IRQ, and fault-driven scheduling events.
- Architecture-neutral scheduler interfaces shared by `riscv64` and `loongarch64`.

## Workflow

1. Inspect existing diffs before editing with `git status --short` and task-scoped `git diff`.
2. Search for domain terms with `rg -n "domain|Domain|ksCurDomain|DomainTime|domain_time|tcbDomain" kernel userspace tools`.
3. Keep domain policy out of shared scheduler and TCB code before touching architecture trap handlers.
4. Replace domain eligibility checks with single-domain behavior: a runnable thread on the current core is schedulable without comparing a domain ID.
5. Keep RISC-V and LoongArch behavior symmetric; single-domain behavior should normally live in shared scheduler code, not architecture-specific branches.
6. If retaining seL4-compatible domain ABI, document and implement it as a single-domain no-op rather than chasing a domain-name-free source tree.

## Validation

Use the smallest useful validation stage:

- Rust-only edits: `cargo fmt --all --check`, then `cargo check`.
- Scheduler-sensitive changes: run focused sel4tests for scheduling, IPC, and multicore behavior on RISC-V first.
- Architecture parity: run matching LoongArch build/test commands when shared scheduler code or LoongArch trap code changed.
- xv6 impact: run a targeted xv6 program such as `tools/run-xv6-user.py forktest` before broad `usertests`.

Do not claim domain scheduling avoidance is complete until temporary diagnostics are cleaned up, compatibility paths are no-ops, and relevant focused validations pass.
