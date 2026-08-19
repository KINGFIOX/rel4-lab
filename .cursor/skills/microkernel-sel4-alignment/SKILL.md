---
name: microkernel-sel4-alignment
description: Keep this Rust RV64 seL4-style microkernel aligned with upstream seL4 behavior and implementation boundaries. Use when changing, reviewing, debugging, validating, or planning kernel code, seL4 ABI/object lifecycle/IPC/scheduler/SMP/VSpace behavior, BKL policy, or milestone docs; prefer matching seL4 semantics over adding features seL4 does not implement, and treat formal verification as out of scope unless explicitly requested.
---

# Microkernel seL4 Alignment

## Overview

Use upstream seL4 as the behavioral baseline and ceiling for this microkernel.
Prefer semantic parity with seL4 over speculative improvements, extra kernel
features, or architecture that seL4 itself does not use.

## Core Policy

- Align kernel behavior with upstream seL4 first: ABI labels, object lifecycle,
  CSpace/MDB/CDT semantics, IPC, scheduling, VSpace, ASID, IRQ, SMP handoff, and
  deletion/finalisation behavior.
- Do not implement kernel features, semantics, or synchronization architecture
  merely because they seem cleaner or more advanced if upstream seL4 does not
  implement or require them.
- Treat formal verification as a non-goal for the current project unless the
  user explicitly asks to work on proofs. Use seL4 proof artifacts and comments
  as design guidance, not as a requirement to build a formal proof stack here.
- If a user asks for behavior beyond seL4, pause and state that it is a scope
  change before implementing it.

## Workflow

1. Identify the relevant upstream seL4 behavior before changing kernel logic.
   Prefer the vendored source under `third_party/sel4-lab/sel4test/kernel`.
2. Compare the current Rust implementation against seL4's algorithm and ABI
   surface. Mark any deviation as either intentional project policy or a bug.
3. Implement the smallest change that moves the Rust kernel toward seL4 parity.
4. Preserve seL4-compatible object sizes and ABI-visible layouts unless the
   project already has an explicit deviation.
5. Validate with focused `sel4test` slices and the repository helper tools.

## BKL And SMP Policy

- Keep the temporary big kernel lock as an intentional seL4-aligned
  synchronization model unless the user explicitly asks to explore BKL removal.
- Do not introduce a fine-grained cross-object lock hierarchy as a default goal.
  seL4 SMP primarily serializes kernel object mutation with a big kernel lock.
- Keep typed Rust wrappers, scoped accessors, and `SpinLock<T>` where they
  remove unsafe global state or express ownership clearly, but do not treat
  those wrappers as a mandate to decompose the BKL.
- Validate BKL coverage and object lifecycle invariants before considering any
  lock decomposition.

## Object Lifecycle Guidance

Mirror seL4 concepts where applicable:

- CTE/MDB/CDT ownership and parent-child relationships.
- `cteDelete`, `cteRevoke`, `finaliseCap`, `emptySlot`, zombie caps, and
  zombie reduction for CNodes and TCBs.
- Preemption points for long delete/revoke operations.
- Remote TCB stall when mutating or finalising a TCB running on another hart.
- TLB and ASID shootdown when VSpace or ASID state changes can leave stale
  translations on any hart.

Do not replace this lifecycle model with a different object graph, reference
counting scheme, or eager recursive destructor unless the user explicitly asks
for a non-seL4 design.

## Useful Source Anchors

Read only what is relevant:

- `kernel/src/object/cnode.c` in upstream seL4 for CTE insert/move/swap,
  revoke/delete, zombie reduction, and slot emptying.
- `kernel/src/object/objecttype.c` for object-specific `finaliseCap` behavior.
- `kernel/include/smp/lock.h` and `kernel/src/smp/ipi.c` for seL4 SMP big
  kernel lock and remote-call behavior.
- `kernel/src/object/tcb.c` for remote TCB stall, TCB deletion, and scheduling
  interactions.
- `kernel/include/arch/riscv/arch/machine.h` for RISC-V local and remote
  `sfence.vma` / ASID flush behavior.

## Validation

Use the smallest validation that covers the changed seL4 surface:

```sh
cargo fmt --all --check
cargo check -p kernel
SEL4TEST_REGEX='...' ./tools/pack-image.py
./tools/run-tests.py
```

For linux-compat-visible effects, add `TIMEOUT=180 ARCH=riscv64 ./tools/run-ltp.py`.
For x86 trap, syscall, or shared scheduler changes, add
`TIMEOUT=60 ARCH=x86_64 SMP=OFF NUM_NODES=1 ./tools/run-hello.py`.
Do not treat `ARCH=x86_64 ./tools/run-tests.py` as a gate.
Do not use QEMU wall-clock timing as a correctness oracle; prefer pass/fail
markers, event ordering, counters, and repeated-run stability.
