# Rust seL4 Status

## Current Kernel Status

The Rust kernel is a seL4-style capability microkernel for the current rel4
scope. It boots through the upstream seL4 elfloader on `qemu-riscv-virt`,
creates a root CNode, BootInfo frame, IPC buffer, user image frames, initial
TCB/VSpace objects, and runs no_std user-space rootservers such as
`xv6-host`.

The kernel is intentionally no longer an MCS/RTOS-compatible seL4 kernel. The
`SchedContext` and `SchedControl` object and invocation surface has been
removed from both kernel and project user-space code. Current scheduling policy
is a simpler cooperative round-robin model:

- A runnable thread is scheduled by FIFO/round-robin runqueue order.
- Explicit `Yield` rotates the current runnable TCB to the tail.
- Scheduler decisions happen after explicit kernel events such as blocking,
  unblocking, faults, suspend/resume, IPC, IRQ delivery, or yield.
- Timer interrupts are retained for clock/IRQ delivery, not for quantum expiry
  or involuntary timeslice preemption.
- Priority and MCP APIs may be accepted for source compatibility, but priority
  values do not affect dispatch, IPC ordering, donation, inheritance, or
  fairness.
- Domain ABI metadata is retained for source compatibility, but every domain
  value is collapsed into one effective global scheduling domain.

Implemented seL4-style areas include:

- BootInfo/rootserver bring-up, root CNode, IPC buffer, user image frames, and
  initial caps for the current ABI subset.
- Untyped, CNode, Frame, PageTable, Endpoint, Notification, TCB, Reply,
  IRQHandler, Domain, and ASID pool objects.
- CSpace lookup, cap copy/mint/move/mutate/delete/revoke, MDB/CDT ownership,
  and seL4-style finalisation/Zombie paths for the covered object lifecycle.
- Endpoint IPC, reply objects, notification wait/signal/binding, selected cap
  transfer paths, and fault IPC.
- RISC-V Sv39 page-table mapping/unmapping, ASID tracking, user fault routing,
  timer IRQ delivery, and basic PLIC external IRQ delivery.
- SMP scaffolding with per-hart boot stacks/current-thread/timer state,
  per-core round-robin runqueues, SBI IPI/RFENCE helpers, remote TCB stall,
  remote FPU-owner release, and a seL4-style big kernel lock for kernel object
  mutation.
- RISC-V FPU lazy ownership and user restore paths aligned with upstream seL4
  for the current single-domain kernel. The detailed closure matrix lives in
  [`fpu-sel4-alignment.md`](fpu-sel4-alignment.md).

## Current seL4 Compatibility Position

This project should currently be described as:

```text
seL4-style capability microkernel
+ seL4 user-source portability subset
+ rel4 cooperative round-robin scheduler
```

It should not be described as:

```text
complete seL4 MCS ABI implementation
complete upstream sel4test-driver compatible kernel
priority/domain/preemptive scheduler compatible with seL4
```

Compatibility policies for the current rel4 scope:

- `SchedContext` / `SchedControl`: removed. User-space in this repository
  should not allocate, configure, bind, donate, or query scheduling contexts.
- Domain scheduling: source-compatible domain caps, BootInfo fields, and
  `DomainSet` may exist, but all requested values are metadata only and map to
  one global effective domain.
- Priority scheduling: `TCBSetPriority`, `TCBSetMCPriority`, and
  `TCBSetSchedParams` may validate basic shape/range/authority, but their
  values must not change runqueue placement or scheduler decisions.
- Preemption: user-space must not depend on timer preemption for correctness,
  progress, ordering, fairness, or timing. If interleaving is needed, use
  explicit yield, blocking IPC, notifications, sleeps, or protocol state.

## Current Validation

Current source-state evidence:

```text
rg -n "SchedContext|SchedControl|sched_context|schedcontrol|SEL4_MIN_SCHED_CONTEXT|SCHED_CONTEXT|CapTag::Sched|ObjectType::Sched" kernel userspace
  no matches

cargo fmt --all --check
  passed
cargo check
  passed
cargo build --release --target riscv64gc-unknown-none-elf -p kernel
  passed
cargo build --release --target loongarch64-unknown-none -p kernel
  passed

SEL4TEST_REGEX='Test that there are tests' ARCH=riscv64 ./tools/pack-image.py
  image ready
ARCH=riscv64 TIMEOUT=60 ./tools/run-tests.py
  timed out after entering the upstream sel4test rootserver without a
  pass/fail banner; this is expected evidence that upstream MCS-rootserver
  compatibility is no longer the active validation baseline.

TIMEOUT=90 ARCH=riscv64 ./tools/run-xv6-user.py echo hello
  PASS: xv6-host: exit(0) pid=1
```

Focused historical MCS sel4test results in older revisions are retired. They
are useful as implementation history, but they no longer prove the current
commit because the MCS scheduling-context ABI has deliberately been removed.

The current preferred validation ladder is:

```sh
cargo fmt --all --check
cargo check
cargo build --release --target riscv64gc-unknown-none-elf -p kernel
cargo build --release --target loongarch64-unknown-none -p kernel
TIMEOUT=90 ARCH=riscv64 ./tools/run-xv6-user.py echo hello
```

For scheduler-visible user-space changes, prefer a targeted xv6 program such
as `forktest` or a focused user program that uses explicit yield/blocking
protocols. For seL4 object/IPC/VSpace work, use the smallest sel4test slice
that does not require the removed MCS/SchedContext surface, or update the test
rootserver first.

## Active Checkpoints

| Area | Current checkpoint |
|------|--------------------|
| Foundation | Boot under the upstream elfloader, rootserver bring-up, cap/object model, Sv39 VSpace support, IRQ/timer delivery, fault IPC, basic scheduler, and standard object finalisation are implemented for the current rel4 ABI subset. |
| Scheduler policy | Cooperative round-robin scheduling is the intended rel4 policy. Priority, domain, MCS budget/refill, and timer-preemptive scheduler semantics are out of scope unless explicitly reintroduced. |
| seL4 ABI subset | Core CNode, Untyped, TCB, Endpoint, Notification, Reply, VSpace, ASID, IRQ, fault, and selected debug invocations remain seL4-style. MCS `SchedContext`/`SchedControl` invocations are removed. |
| User-space portability | Repository user-space should build around explicit synchronization and should not rely on multiple domains, priority ordering, or preemption. Compatibility calls may remain only when they help the same source run on seL4 and rel4. |
| CSpace/object lifecycle | CTE/MDB/CDT operations, final cap handling, Zombie remainders, CNode/TCB finalisation, endpoint/notification cleanup, reply cleanup, VSpace/ASID metadata, and IRQ cap publication use seL4-style ordering for the covered paths. |
| RISC-V FPU alignment | The RISC-V FPU path remains tracked against upstream seL4 for the single-domain rel4 kernel. See `fpu-sel4-alignment.md` for the detailed requirement matrix and evidence gates. |
| SMP/BKL | The big kernel lock remains the intentional seL4-style kernel-object mutation boundary. Per-core runqueues, remote TCB stall, remote FPU-owner release, IPI/RFENCE helpers, and all-hart ASID/TLB invalidation scaffolding exist, but broad stress coverage is still pending. |
| xv6 compatibility | The xv6 user-space stack is the main current runtime smoke path. It should use explicit IPC/yield/blocking behavior rather than seL4 priority/domain/preemptive scheduler assumptions. |

## Historical Notes

Older milestones recorded a period where the Rust kernel targeted the seL4 MCS
ABI and passed focused MCS sel4test slices involving `SchedContext`,
`SchedControl`, refill queues, consumed accounting, timeout faults, donation,
and `SchedContext_YieldTo`. That line of work is now retired for the current
rel4 scope.

Earlier bring-up milestones such as standalone UART boot, initial cap creation,
first `seL4_Call`, early `Untyped_Retype`, initial non-MCS sel4test sweeps, and
focused MCS scheduler slices should be read as historical implementation
context, not as current validation claims.

## Remaining Kernel Work

The remaining kernel work is now about tightening the rel4 subset rather than
recovering full MCS behavior:

1. Keep removing stale compatibility code whose only purpose is MCS,
   priority-based dispatch, multi-domain scheduling, or timer preemption.
2. Harden CSpace/object lifecycle ordering for Delete/Revoke/finalise,
   especially exposed/remainder continuation cases and cross-object cleanup.
3. Broaden IPC cap-transfer and endpoint-unwrapping coverage.
4. Strengthen SMP coverage around BKL assertions, remote TCB stall, ASID
   recycling, all-hart TLB shootdown, affinity migration, and cross-hart
   cleanup.
5. Maintain seL4 alignment for object, IPC, VSpace, IRQ, FPU, and debug ABI
   behavior inside the explicit rel4 subset.
6. Keep user-space portable by avoiding reliance on domains, priority, or
   preemption, and by making ordering/progress explicit in protocols.
