# Kernel

The `kernel` crate is a no_std seL4-style capability microkernel. Shared code
is architecture-neutral. Each ISA backend is selected at compile time with
`cfg(target_arch)` and re-exported as `arch::current`
(`kernel/src/arch/mod.rs`). Formal verification
is not implemented.

Runnable today: `riscv64gc-unknown-none-elf` on QEMU `virt`, booted through the
upstream seL4 elfloader, and `x86_64-unknown-none` on QEMU `pc` with a
Multiboot kernel plus a user initrd. linux-compat and a narrow unicore
sel4test slice run on both.

## Layout

```text
kernel/src/
|-- abi/           # BootInfo, syscall numbers, fault labels, constants
|-- api/           # syscall dispatch, invocations, CSpace lookup, IPC
|-- object/        # caps, CTE/MDB, TCB, endpoint, notification, reply, ASID, IRQ
|-- kernel/        # shared boot and SMP / BKL
|-- machine/       # shared console only
    `-- arch/
    |-- riscv64/   # sel4_arch, machine, kernel (trap.S), object, plat, smp
    `-- x86_64/    # same module names; trap.S, x2APIC timer, 4-level VSpace
```

Each arch crate follows the seL4 compile-time split:

| Module | Role |
|--------|------|
| `sel4_arch` | `UserContext` accessors, arch invocation IDs, `ObjectType`, `VspaceRoot` |
| `machine` | paging bits, TLB, FPU, IRQ/PLIC or x2APIC |
| `kernel` | boot, trap, `TrapScratch` |
| `object` | VSpace map/unmap |
| `plat` | QEMU virt or pc99 constants |
| `smp` | IPI / remote TLB |

Shared MI code is allowed to call `UserContext::{cap_reg,msg_info,mr,set_mr}`,
`switch_vspace`, `machine::tlb::{flush_all,flush_asid,flush_vaddr}`, and
`kernel::smp::{current_core_id,send_ipi,...}`. RISC-V words such as `satp` and
`sfence` stay under `arch/riscv64/`.

## Objects

`CapTag` in `kernel/src/object/cap.rs`:

`Null`, `Frame`, `Untyped`, `PageTable`, `Endpoint`, `Notification`, `Reply`,
`CNode`, `AsidControl`, `Thread`, `AsidPool`, `IrqControl`, `IrqHandler`,
`Zombie`, `Domain`.

`Untyped_Retype` object IDs are arch-local:

- RISC-V (`arch/riscv64/sel4_arch/object_type.rs`): Untyped, TCB, Endpoint,
  Notification, CapTable, GigaPage, 4K, MegaPage, PageTable.
- x86_64 (`arch/x86_64/sel4_arch/object_type.rs`): same common objects, then
  PDPT, PML4, 4K, large page, page table, page directory.

CSpace operations live in `object/cnode.rs` and `object/mdb.rs` (insert, copy,
mint, move, mutate, delete, revoke, zombie reduction). Invocation entry points
are `api/invocation.rs::handle_*`.

## Invocations

Shared labels in `api/invocation.rs` (`InvocationLabel`, 1–32):

| ID | Label |
|----|--------|
| 1 | UntypedRetype |
| 2–16 | TCB read/write/copy registers, configure, priority/MCP/sched params, IPC buffer, space, suspend, resume, bind/unbind notification, TLS base, flags |
| 17–25 | CNode revoke/delete/cancel/copy/mint/move/mutate/rotate/save-caller |
| 26–29 | IRQ issue/ack/set/clear |
| 30–32 | DomainSet, DomainScheduleConfigure, DomainScheduleSetStart |

`handle_domain` accepts only `DomainSet`. Labels 31 and 32 return
`IllegalOperation`.

RISC-V arch labels (`arch/riscv64/sel4_arch/invocation.rs`) start at 33:
PageTable map/unmap, Page map/unmap, PageGetAddress, ASID control/pool, IRQ
trigger.

x86_64 arch labels (`arch/x86_64/sel4_arch/invocation.rs`) start at 33 with
the seL4 x86 paging objects (PD, PT, page, PDPT, PML4, then ASID and IRQ
trigger). Those IDs are not interchangeable with the RISC-V sequence.

Shared `handle_frame` / `handle_page_table` dispatch the current arch’s
`PAGE_*` and mapped-table labels. On x86_64 that includes page map/unmap,
`PageTable` (2 MiB coverage), `PageDirectory` (1 GiB), and `PDPT` (512 GiB).
`PML4_Map` stays `IllegalOperation` (the PML4 is the VSpace root). All x86
paging `ObjectType`s still create `Cap::new_page_table`, and map checks the
invocation coverage so a PT cannot be installed in a PDPT slot.

## Syscalls

`abi/syscall.rs` (`SyscallNumber`, non-MCS api-master):

`Call=-1`, `ReplyRecv=-2`, `Send=-3`, `NBSend=-4`, `Recv=-5`, `Reply=-6`,
`Yield=-7`, `NBRecv=-8`, then debug `-9`…`-15`.

RV64 dispatch is in `arch/riscv64/kernel/trap.rs`. Handlers:

- `Call` / `Send` / `NBSend` / `Recv` / `NBRecv` / `Reply` / `ReplyRecv` →
  `api/syscall.rs` and `api/ipc.rs`
- `Yield` → `tcb::rotate_to_tail`
- `DebugPutChar` → `machine::console::putc`
- `DebugNameThread` names a TCB
- `DebugCapIdentify` returns the cap tag for a CPtr
- `DebugHalt` halts
- `DebugSendIpi` halts with “not supported”
- `DebugDumpScheduler` and `DebugSnapshot` are success no-ops

Recv ignores the reply register. `seL4_Reply` and `seL4_ReplyRecv` use the
current thread's `tcbCaller` slot. A reply cap is a non-retypable cap that
points at a TCB (`tcbReply` master, `tcbCaller` derived), matching non-MCS
seL4. `CNodeSaveCaller` moves the derived caller cap.

`do_send` handles Notification, Endpoint, Reply, and Thread
(`TcbSetFlags` only via `handle_thread_send`). Other cap tags are dropped
with no error reply.

## Scheduler

Per-core FIFO runqueues in `object/tcb.rs`. `schedule()` dequeues the head
runnable TCB on the current core. Priority is not consulted.

Every trap returns through `kernel_exit` in the arch `trap.rs`
(`arch/riscv64` and `arch/x86_64` each have a copy):

1. If `TCBResume` set `continue_current_once` on the current TCB, resume it.
2. Otherwise enqueue the current TCB if it is still runnable.
3. Dequeue the runqueue head. On a different TCB, switch VSpace and restore
   that context.
4. If the queue is empty and current is not runnable, switch `current` to
   that core's idle TCB, install the kernel VSpace, drop the kernel lock,
   and wait (`WFI` / `HLT`) until something is woken.

Each core has a static idle TCB created at boot (`create_idle_threads`),
matching seL4 `ksIdleThread`. Idle is never enqueued. The idle TCB context
is filled as upstream does (`idle_thread` PC, kernel privilege, FPU
disabled) but is not restored through `sret`/`sysret`; the wait loop stays
in kernel mode.

Timer interrupts therefore switch threads whenever another runnable TCB is
queued on the same core. Non-blocking syscalls take the same path. This is
not cooperative, and it is not priority preemption. There is no per-TCB
timeslice counter.

`TIME_SLICE_TICKS` and the `SEL4_MIN_BUDGET_*` / refill constants in
`abi/constants.rs` are unused by the scheduler. The RV64 timer in
`arch/riscv64/kernel/trap.rs` uses `TIMER_INTERVAL_TICKS` (5000) and a
synthetic IRQ interval (20000) only to reprogram the SBI timer and raise a
trap; it does not decrement a quantum.

## Domain and priority

`NUM_DOMAINS` is 1 (`abi/constants.rs`). `DomainSet` checks
`domain < NUM_DOMAINS` and that the extra cap is a Thread, then returns
success without writing a domain field (`handle_domain`).

`TCBSetPriority`, `TCBSetMCPriority`, and `TCBSetSchedParams` check message
shape, that the authority cap is a Thread, and that values are `<= 255`.
`Tcb` has no priority field. `NUM_PRIORITIES` (256) is unused.

`Tcb.affinity` selects which core’s runqueue the thread is eligible for.
`tcb::set_affinity` exists and releases a remote FPU owner on migration.
No `TcbSetAffinity` invocation is wired; boot sets affinity to
`current_core_id()`.

## IPC and faults

Endpoint send/receive/call, notifications, non-MCS reply caps, and selected
cap transfer live in `api/ipc.rs` plus `object/{endpoint,notification,reply}.rs`.

Fault labels (`abi/fault.rs`): CapFault=1, UnknownSyscall=2, UserException=3.
`Timeout` and `VmFault` both encode as 5.

linux-compat user programs enter the host as `UnknownSyscall` fault IPC.

## VSpace

Shared boot and ASID code use `VspaceRoot` and
`arch::current::object::vspace`. TLB invalidation goes through
`arch/*/machine/tlb.rs`.

RV64 is Sv39: one `PageTable` object type, 4K / 2M / 1G leaves
(`arch/riscv64/machine/paging.rs`, `object/vspace.rs`). Hardware programming
uses `satp` and `sfence.vma` only inside that backend.

x86_64 paging types are PML4 / PDPT / PD / PT. `machine/paging.rs` uses a
4-level walk (`ROOT_LEVEL = 3`). `USER_TOP` is the canonical user half
(`256 << (12 + 9 * 3)`). Boot maps a kernel 1G window and walks all four
levels for user 4K maps. `prepare_user_frame_map` / `unmap_user_frame` /
`prepare_user_page_table_map` install or clear PTEs and `invlpg`. User
`PageTable` / `PageDirectory` / `PDPT` map invocations share those helpers
and check coverage alignment. `PML4_Map` is illegal.

## SMP

`MAX_NUM_NODES` comes from the `kernel_num_nodes` cfg (default 1; packer can
set 2–8). Shared state is in `kernel/src/kernel/smp.rs`:

- `KernelLockGuard` — big kernel lock for object mutation
- `current_core_id`, `init_current_cpu`, `publish_kernel_vspace`
- `remote_tcb_stall`, `remote_fpu_owner_release`, `wake_core`

`TrapScratch` is arch-local (`arch/*/kernel/trap_scratch.rs`). RV64 IPI and
remote `sfence.vma` use SBI (`arch/riscv64/smp`). x86 uses x2APIC IPI,
an AP trampoline at physical `0x8000`, and IPI TLB shootdown
(`SUPPORTS_REMOTE_IPI` / `SUPPORTS_REMOTE_TLB_FLUSH` are true).

## Boot

`kernel/src/kernel/boot.rs::bringup_rootserver` builds the initial CNode,
maps BootInfo and the IPC buffer, loads the user image, and restores the
rootserver TCB.

Root CNode slots (`abi/bootinfo.rs::RootCNodeCapSlot`): TCB=1, CNode=2,
VSpace=3, IRQControl=4, ASIDControl=5, ASID pool=6, BootInfo=9, IPC buffer=10,
Domain=11. On the RISC-V profile, `IoPortControl` and `IoSpace` stay null.

## RISC-V backend

`arch/riscv64/` implements trap entry (`trap.S` + `kernel/trap.rs`), SBI
timer, PLIC (`machine/plic.rs`), Sv39, lazy FPU (`machine/fpu.rs`), and
QEMU virt constants (`plat`). This is the linux-compat and sel4test host.

## x86_64 backend

`arch/x86_64/kernel/boot.rs` has a Multiboot header, 32→64 trampoline
(enables NX and SSE), and early identity maps, then calls
`bringup_rootserver`. `arch/x86_64/trap.S` plus `kernel/trap.rs` install a
per-core GDT/TSS, a shared IDT, `syscall`/`sysret`, and `iret` for faults,
the LAPIC timer, IOAPIC IRQs, and IPI. `#PF` is delivered as VM-fault IPC
when a fault endpoint exists. UnknownSyscall uses the seL4 x86_64 19-word
layout (RAX…R15, FaultIP, SP, FLAGS, Syscall); FaultIP is already the
instruction after `syscall`. `machine/irq.rs` programs x2APIC plus the
IOAPIC. `IRQIssueIRQHandler` issues the LAPIC timer only; IOAPIC pins use
`GetIOAPIC`. Console output is COM1 `0x3f8`. Lazy FPU uses `#NM`, `CR0.TS`, and
`fxsave`/`fxrstor`. Gates: `tools/run-hello.py`, a narrow unicore
`pack-image.py` + `run-tests.py` slice, and `ARCH=x86_64 ./tools/run-ltp.py`.

## Comments that are not true

`handle_thread` still says the kernel has no scheduler. Suspend, resume,
yield, and `kernel_exit` all mutate the runqueue. Trust the functions, not
that comment.
