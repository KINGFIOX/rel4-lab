# Rust seL4 Status

## Current Kernel Status

The Rust kernel boots under the upstream seL4 elfloader on
`qemu-riscv-virt`, runs the unmodified `sel4test-driver` rootserver, creates
user TCBs/VSpaces/CSpaces, forwards faults through seL4 IPC, and implements the
MCS seL4 ABI as the baseline kernel interface.

MCS is part of the kernel. There is no active `kernel_mcs` source/build split;
users who do not need MCS simply do not create or bind MCS objects. The
remaining work is about ABI, object lifetime, IPC, and scheduling-context
fidelity, not choosing between MCS and non-MCS kernels.

Implemented areas include:

- BootInfo/rootserver bring-up, root CNode, IPC buffer, and user image frames.
- Untyped, CNode, Frame, PageTable, Endpoint, Notification, TCB, Reply,
  SchedContext, IRQHandler, and ASID pool objects.
- CSpace lookup, cap copy/mint/move/mutate/delete/revoke, MDB/CDT ownership,
  and the finalisation paths covered by the current tests.
- Endpoint IPC, explicit MCS reply objects, notification wait/signal/binding,
  selected cap transfer paths, and fault IPC.
- RISC-V Sv39 page-table mapping/unmapping, ASID tracking, user fault routing,
  timer IRQ delivery, basic PLIC external IRQ delivery, and preemption points.
- MCS scheduling-context binding/reconfiguration, refill queues, budget
  accounting, timeout faults, notification donation, `SchedContext_YieldTo`,
  consumed-time reporting, DomainSet support for the enabled single-domain
  tests, and a first SMP scheduler pass with per-hart boot stacks, per-core
  run queues, timer state, and secondary harts admitted to the scheduler.

## Current Validation

The active validation baseline is the MCS kernel ABI. Older non-MCS checkpoints
are historical only.

```text
Focused single-core MCS slice:
62/62 tests passed under qemu-riscv-virt with SMP=OFF.

Broader single-core MCS run:
best recorded result is 162/165 tests passed. The remaining failures are the
known unsupported FPU tests. A later consumed-accounting run reached 161/165
because SCHED0011 exceeded the QEMU timing tolerance by about 2 ms; that is
tracked as simulation timing jitter, not the main MCS conformance frontier.

Focused SMP MCS slices:
focused `SMP=ON NUM_NODES=2` runs pass for SCHED0022,
SCHED_CONTEXT_0014, MULTICORE0001, MULTICORE0002, MULTICORE0003,
MULTICORE0004, MULTICORE0005, BIND0001..0006, INTERRUPT0002..0006,
IPC0001..0004, IPC0025..0026, and IPC1001..1004.

Latest focused two-hart regression:
the current `INTERRUPT000[2-6]|BIND000[1-4]|BIND00[56]|IPC000[1-4]|
IPC002[56]|IPC100[1-4]|MULTICORE000[1-5]|SCHED_CONTEXT_0014` image
passes 32/32 tests with 0 disabled.

Focused receive CapFault/notification slice:
the two-hart `SYSCALL001[023]|IPCRIGHTS0002|NBWAIT0001|BIND000[1-4]|
BIND00[5-6]|IPC000[1-4]|IPC002[5-6]` image passes 22/22 tests with
0 disabled.

Focused Delete/Revoke-adjacent SMP slice:
the `CNODEOP000[1-8]|RETYPE000[0-2]|CSPACE0001|FRAMEEXPORTS0001|
VSPACE000[0-6]|PAGEFAULT000[1-5]|PAGEFAULT100[1-5]` image passes
31/31 tests with 0 disabled on two harts.

Focused finalisation/notification-SC SMP slice:
the `BIND006|SCHED_CONTEXT_0014|RETYPE000[0-2]|CNODEOP000[1-8]`
image passes 15/15 tests with 0 disabled on two harts.

Focused endpoint teardown slices:
the single-core `CANCEL_BADGED_SENDS_000[12]|CNODEOP000[67]|IPC0010|
SCHED_CONTEXT_0009|SCHED_CONTEXT_0010|SCHED_CONTEXT_0011|
SCHED_CONTEXT_0012|SCHED_CONTEXT_0013|RETYPE000[0-2]` image passes
15/15 tests with 0 disabled, and the two-hart `CANCEL_BADGED_SENDS_000[12]|
CNODEOP000[67]|IPC0010|RETYPE000[0-2]|BIND006|SCHED_CONTEXT_0014`
image passes 12/12 tests with 0 disabled.

Focused reply-object receive-binding slice:
the two-hart `IPC0016|IPC002[3-7]|SCHED_CONTEXT_0009|
SCHED_CONTEXT_0010|SCHED_CONTEXT_0011|SCHED_CONTEXT_0012|
SCHED_CONTEXT_0013` image passes 13/13 tests with 0 disabled.

Focused current IPC-buffer access slice:
the two-hart `REGRESSIONS0001|THREADS000[45]|FRAMEEXPORTS0001|
FRAMEDIPC0003|IPCRIGHTS000[1-3]|SYSCALL001[0-3]|SYSCALL0018|
NBWAIT0001|IPC002[56]` image passes 18/18 tests with 0 disabled.

Focused bound-notification endpoint slices:
the single-core `BIND000[1-6]|NBWAIT0001|SCHED_CONTEXT_000[67]` image
passes 9/9 tests with 0 disabled, and the two-hart `BIND000[1-6]|
NBWAIT0001|SCHED_CONTEXT_000[67]|SCHED_CONTEXT_0014` image passes
10/10 tests with 0 disabled.
```

The focused single-core MCS slice covers:

```text
BIND005, BIND006,
FRAMEDIPC0001, FRAMEDIPC0002,
INTERRUPT0002..0006,
IPC0011..0027,
IPCRIGHTS0001..0003,
NBWAIT0001,
SCHED0007..0014, SCHED0016..0019,
SCHED_CONTEXT_0001, SCHED_CONTEXT_0002, SCHED_CONTEXT_0003,
SCHED_CONTEXT_0005..0013,
SYSCALL0018,
THREADS0004, THREADS0005,
TIMEOUTFAULT0001..0003
```

Additional single-core MCS-compatible coverage includes:

```text
BIND0001..0004,
CANCEL_BADGED_SENDS_0001..0002,
CNODEOP0001..0008,
CACHEFLUSH0004, CSPACE0001, DOMAINS0001..0003,
FRAMEDIPC0003, FRAMEEXPORTS0001,
IPC0001..0004, IPC0010, IPC1001..1004,
PAGEFAULT0001..0005, PAGEFAULT1001..1005,
REGRESSIONS0001,
RETYPE0000..0002,
SCHED0002..0005, SCHED0020,
SERSERV_CLIENT_001..005, SERSERV_CLI_PROC_001..005,
SYNC001..004,
SYSCALL0000..0002, SYSCALL0004..0006,
SYSCALL0010..0015, SYSCALL0017..0018,
TLS0001..0002,
TRIVIAL0000..0002,
VSPACE0000, VSPACE0002..0006
```

The focused SMP MCS pass now boots a `NUM_NODES=2` image on two QEMU harts.
Secondary harts use independent boot stacks, switch from the elfloader page
table to the kernel root `satp`, initialise per-hart trap/timer state, and run
the per-core scheduler idle loop. `TCB_SetAffinity` / per-core ready queues are
covered by the focused `SCHED0022` run (`Node 0 of 2`, 3 tests passed). Remote
current-TCB suspend/finalise/affinity changes now kick the owning hart with an
IPI, and idle harts switch back to the published kernel `satp` before waiting
so they do not retain a destroyed user VSpace. This is still a first-stage SMP
scheduler: a seL4-style big kernel lock is the intentional global kernel-object
mutation boundary, and broader cross-core preemption/load tests remain to be
enabled.

SMP state protection now follows upstream seL4's BKL model directly. Fixed-size
objects and global object tables no longer carry independent lock tables for
Endpoint/Notification wait queues, CSpace/CDT, IRQ handlers, SchedContext,
Reply, TCB state, runqueues, VSpace roots, ASID state, boot page-table state,
or pending revoke state. Rust code uses `BklObjectGuard` marker guards and
`BklCell<T>` storage to assert that these paths run during single-core boot or
while the seL4-style BKL is held; they are not fine-grained object locks and do
not define an object lock-ordering architecture.

TCB, Reply, SC, IPC, fault-IPC, endpoint, notification, CSpace, VSpace, ASID,
IRQ, pending-revoke, and runqueue helper APIs still centralise their snapshots
and writes so raw object fields are not open-coded throughout the syscall path.
The difference is that those helper sections now document the BKL boundary
rather than acquiring per-object spinlocks. The userspace `sel4-user`
IPC-buffer pointer remains an `AtomicPtr`, and rootserver TCB / initial
sched-context storage still use transparent `UnsafeCell` wrappers so their
cap/object addresses remain ABI-stable without `static mut`.

Trap, boot, and idle scheduler returns now match the upstream handoff shape more
closely: the primary boot path takes the BKL before releasing secondary harts,
and selected user contexts go through the locked restore trampoline so the BKL
is released at the final restore boundary.
Delete/Revoke now snapshot the target slot or revoke leaf under a short CSpace
helper section, run object finalisation while the CTE remains populated, and
then take the CSpace marker guard again for seL4-style `emptySlot` cleanup.
CNode and TCB finalisation now return seL4-style Zombie remainders for their
embedded CTE ranges, and Delete/Revoke reduces those zombies through the same
per-slot `cteDelete` path before `emptySlot` cleanup. Final Thread-cap
destruction runs only for the final cap and clears TCB-owned wait, reply,
IPC-buffer, fault, scheduling-context, notification, and space-root mirrors after
unlinking external wait queues, run queues, bound notifications, sched-context
bindings, yield-to state, and reply state. Endpoint sender/receiver waiter pop
state checks now live behind Endpoint-owned helpers, including IPC and
fault IPC rendezvous paths, and Endpoint/Notification raw queue accessors are no
longer exposed outside their object modules. Endpoint finalisation and
CancelBadgedSends now centralise endpoint waiter cleanup, unlink receive-side
reply objects before clearing receive state, clear transient sender/fault
fields, and leave aborted fault-handler sends inactive instead of requeueing
them. Bound-notification signaling now detaches endpoint receive waiters through
Notification/Endpoint helper sections under the BKL, then unlinks receive-side
reply objects before completing the notification wakeup. Active
bound-notification receive paths now consume badge/state through
Notification-owned helpers instead of open-coding the state mutation in IPC.
Notification finalisation now unbinds any bound scheduling context, clears the
bound-TCB link, and drains waiters from a single notification detach snapshot.
Receive syscall dispatch now raises receive-phase CapFaults for invalid receive
caps and notifications bound to another TCB instead of returning an empty
message.
Receive-side reply binding now clears receive-only link metadata on successful
bind and refuses to overwrite a reply object already owned by another valid TCB;
contested reply caps fail closed to the implicit reply path instead of
clobbering live reply or receive ownership. Reply finalisation now shares the
owner cleanup path for blocked reply and receive owners. SchedContext unbind and
TCB sched-context replacement now clear TCB mirrors only when the TCB still names
the SC being detached. The initial rootserver SchedContext now reaches the same
final-cap cleanup as retyped SchedContexts despite its static boot storage, so
root TCB binding and tracked-SC state do not survive as a boot-storage
exception. Donation paths now rely on the sched-context helpers
instead of target-TCB sched-context snapshots when deciding whether to donate.
SchedContext binding now rejects binding an unreleased SC to a blocked TCB,
matching seL4's `SchedContext_Bind` decoder.
`TCB_SetSchedParams` sched-context binding now refuses to overwrite a
TCB mirror that no longer names the target SC or zero. Frame mapping metadata
now uses `capFMappedASID` as the mapped/unmapped sentinel, so VA 0 mappings
remain unmap/finalise-reachable, and `Page_Map` fails closed if no ASID can be
recorded. `ASIDPool_Assign` now rechecks the destination VSpace cap under the
CSpace marker guard before allocating and publishing the ASID, and ASID-pool
assignment refuses cross-pool duplicate root mappings. The CSpace helper path
also covers IPC cap transfer, Untyped Retype publication, ASID/IRQ cap
installation, CNode
Copy/Mint/Move/Mutate/Rotate/
SaveCaller, Frame map/unmap cap metadata updates, and the IRQ notification
internal slot. IRQ handler issuance now atomically reserves the IRQ active
state before publishing the handler cap and rolls that reservation back if the
destination slot cannot be populated. IRQ notification binding now rechecks the
source Notification cap under the CSpace marker guard before relinking the
internal IRQ notification slot, and `IRQHandler_SetNotification` now reports
`InvalidCapability` when that revalidation or active-IRQ check fails. Frame
`Page_Map`/`Page_Unmap` now revalidate the frame cap slot and keep CSpace and
VSpace marker guards live across the PTE mutation plus mapped ASID/address
metadata publication or clear. VSpace user PTE map/unmap/prune operations assert
BKL coverage before walking or mutating page tables, while ASID table and boot
page-table state live in `BklCell`. PageTable finalisation now removes the ASID
route before reclaiming user page tables. `ASIDPool_Assign` now allocates the
first free ASID from the selected pool, the initial-thread ASID pool is backed
by a real boot ASID-pool object instead of a null-pointer placeholder, and ASID
pool deletion no longer special-cases pool 0. Invocation decode now accepts the
generated seL4 MCS ABI labels exactly and no longer carries a dynamic
label-shift compatibility layer for VSpace, ASID, IRQ, Domain, TCB, or CNode
operations. `TCB_ReadRegisters` and `TCB_WriteRegisters` now reject self-target
and invalid/truncated register-count cases that upstream seL4 rejects.
`TCB_BindNotification`/`TCB_UnbindNotification` now reject already-bound TCB,
unbound TCB, rights-limited Notification, queued Notification, or already-bound
Notification cases instead of silently replacing or ignoring the binding state.
TCB IPC-buffer updates now reject non-frame/device frame caps and unaligned
buffer VAs, matching seL4 `checkValidIPCBuffer`.
TCB space updates now validate CSpace/VSpace roots after `updateCapData` and
`deriveCap`, reject unmapped VSpace roots, and use seL4's `IllegalOperation`
class for invalid root caps instead of accepting or pre-classifying them.
Invocation decoders that require extra caps now require a mapped caller IPC
buffer, and optional TCB extra caps resolve CPtr 0 through CSpace instead of
treating it as an implicit null cap.
Invocation dispatch now clamps the message length to the in-register MR count
when the caller has no IPC buffer, matching upstream seL4's slowpath before
decoders inspect long argument lists.
`seL4_DebugNameThread` now looks up a TCB cap, reads the NUL-terminated name
from the caller's IPC buffer, and updates the TCB debug name instead of
silently accepting every request.
`seL4_DebugHalt` and RISC-V `seL4_DebugSendIPI` now halt instead of returning
as successful no-ops, matching upstream debug-build behavior.
Delete/Revoke has started its seL4 ordering pass by snapshotting CTE/MDB state
under the CSpace marker guard, finalising while the source CTE remains
populated, then routing CNode/TCB finalisation through Zombie remainders,
`reduceZombie`, and only then `emptySlot`. Boot-allocated rootserver TCB/CNode
objects now use the same Zombie remainder path instead of skipping it because
their storage lives in the kernel ELF window. Full Delete/Revoke/finalise
hardening still needs a strict CSpace/object finalisation-ordering audit and
deeper exposed/remainder continuation checks, because finalisation crosses into
TCB, Endpoint, Notification, SchedContext, Reply, IRQ, VSpace, and ASID state.
The BKL is now treated as intentional seL4-aligned synchronisation. Follow-up
SMP work should tighten BKL coverage assertions, remote-call/remote-TCB stall
handling, ASID recycling stress, broader targeted TLB shootdown, and the locked
restore-user-context handoff without reintroducing object locks as a BKL-removal
architecture.

## Active Checkpoints

The old boot/object-model milestone list is no longer useful as a working
tracker. Those foundations are collapsed into the first row below; detailed
history belongs in git history rather than this status document.

| Area | Current checkpoint |
|------|--------------------|
| Foundation | Boot under the upstream elfloader, rootserver bring-up, cap/object model, Sv39 VSpace support, IRQ/timer delivery, fault IPC, basic scheduler, and standard object finalisation are done for the covered sel4test paths. |
| MCS as baseline ABI | `kernel_mcs` gating has been removed from the kernel, and image packing defaults to `MCS=ON`. |
| Single-core MCS IPC/scheduler | SC binding/reconfiguration, timeout faults, notification donation, `YieldTo`, MCS IPC donation, reply-cap cleanup, `NBSendRecv`, `NBRecv`, endpoint IPC rights, TCB IPC-buffer validation, and scheduler tests through the focused slice pass. |
| SchedContext layout/refills | `SchedContext` uses the 128-byte seL4 core layout, a two-refill minimum, extra refill slots from larger SC objects, head/tail circular refill queues, and finalisation unregisters dead SCs from release scans. |
| Consumed accounting | `SchedContext_Consumed`, timeout fault consumed values, and `YieldTo` consumed reporting use update-and-clear microsecond semantics with one-word success replies for direct SC invocations. |
| Broader single-core ABI | CNode operations, endpoint cleanup, VSpace/untyped/fault paths, syscall register preservation, TLS/sync helpers, and serial-server IPC pass in focused slices. |
| SMP scheduler first pass | Per-core `SchedControl` BootInfo exposure, per-hart boot stacks/current-thread/timer slots, per-core ready queues, SBI IPI/RFENCE wrappers, remote current-TCB wakeup/preemption for TCB and SC unbind/finalise paths, idle switchback to the published kernel `satp`, runqueue affinity/priority/SC snapshots under BKL coverage, stale-priority ready-queue dequeue fallback, stale ready-head filtering, SchedContext tracked list, ASID table with all-hart TLB flush on ASID deletion, boot page-table pool, VSpace user PTE mutation, IRQ handler table, fixed-layout Endpoint/Notification wait queues, pending CNode_Revoke continuation state, locked restore trampoline handoff, Delete/Revoke finalise-before-empty CTE snapshots, CSpace lookup walks and cap snapshots, and CSpace/CDT CTE/MDB updates under the seL4-style BKL. Object-state storage uses `BklCell` and marker guards rather than external object locks or object lock-ordering rules. Focused `NUM_NODES=2` SMP MCS runs now pass `SCHED0022`, `SCHED_CONTEXT_0014`, `MULTICORE0001..0005`, `BIND0001..0004`, `BIND005..006`, `INTERRUPT0002..0006`, `IPC0001..0004`, `IPC0025..0026`, and `IPC1001..1004`. |

## Historical Notes

The recorded non-MCS line is retained only as background:

```text
121/125 enabled RV64 single-core non-MCS tests passed after the
no-kernel-floating-point cleanup. FPU0000, FPU0002, FPU0003, and FPU0004 fail
because kernel floating-point/FPU flag support is intentionally absent.
```

Earlier bring-up milestones such as standalone UART boot, initial cap creation,
first `seL4_Call`, early `Untyped_Retype`, and the first enabled sel4test sweep
are considered retired. They should not drive current planning.

## Remaining Kernel Work

The remaining kernel work is not about basic boot or choosing an MCS build
mode. MCS is part of the kernel; the frontier is conformance beyond the current
test slices:

1. MCS fidelity: broaden object, IPC, reply, and scheduling-context lifecycle
   coverage; tighten refill reconfiguration semantics; and test MCS IPC outside
   the current `IPC0011..0027` slice.
2. Real-time scheduling precision: keep gross regressions visible, but do not
   treat QEMU/software-simulation timing jitter as the primary blocker.
3. True SMP hardening: broader affinity/preemption tests such as SCHED0021
   once it is enabled upstream, repeated cross-hart yield/cleanup stress,
   cross-hart load, MCS IPC donation on multiple harts, BKL coverage assertions,
   seL4-style remote-call/remote-TCB stall handling, ASID table and boot-PT pool
   stress, VSpace/CSpace BKL coverage, pure CTE/MDB publication coverage,
   Delete/Revoke/finalise CSpace/object ordering, and cross-hart TLB
   shootdown stress while preserving the big kernel lock as the intended
   seL4-aligned synchronization model.
4. Finalisation/Zombie fidelity beyond the paths covered by the enabled tests:
   harden Delete/Revoke's current `finaliseCap` -> Zombie/remainder ->
   `reduceZombie` -> `emptySlot` path with stricter exposed/remainder
   continuation and cross-object ordering audits.
5. Multi-domain scheduling beyond the currently enabled single-domain cases.
6. Broader IPC cap-transfer and endpoint-unwrapping edge cases.
