# FPU

Both backends implement lazy FPU ownership without MCS handoff. RISC-V lives
in `kernel/src/arch/riscv64/machine/fpu.rs` (`f0..f31` / `fcsr`). x86_64 lives
in `kernel/src/arch/x86_64/machine/fpu.rs` (`#NM`, `CR0.TS`, `fxsave` /
`fxrstor`). Shared TCB flags, trap restore, and SMP remote release call the
current-arch module.

## What the code does

Per core (`MAX_NUM_NODES` wide atomics):

- `FPU_OWNER` — TCB whose `f0..f31` / `fcsr` are live in hardware
- `FPU_ACCESS_ENABLED` — software shadow of user FPU access

`init_current_core` clears the owner, sets `sstatus.FS` clean, writes
`fcsr=0`, then disables the access shadow.

`enable_access` / `disable_access` only store the shadow. They do not write
`sstatus`. Supervisor `FS` is cleared at explicit boot/trap boundaries
(`clear_supervisor_access`). User restore writes the TCB's saved `sstatus`.

`lazy_restore(thread)`:

- if the TCB has FPU disabled, drop access and return
- if this core already owns `thread`, enable the shadow
- otherwise `switch_local_owner`: save the old owner, load the new one

`release` / `release_on_current_core` drop ownership. A remote owner goes
through `kernel/src/kernel/smp.rs::remote_fpu_owner_release` and does not
deschedule that TCB.

Save/load cover `f0..f31` and `fcsr`. The `asm!` blocks omit `nomem` so the
compiler treats the TCB FPU image as memory, and they run inside a
`TcbRef::with_context`/`with_context_mut` borrow of the owning TCB.

`TCBSetFlags` (`api/invocation.rs`) can set the seL4 FPU-disabled flag.
Setting it releases a live owner. Re-enabling the current TCB calls
`lazy_restore` before return.

There is no MCS scheduling-context FPU handoff and no multi-domain
`prepareSetDomain` path. `NUM_DOMAINS` is 1.

## Source checks

These scripts read the current kernel; they are the FPU “matrix”, not a
saved QEMU log.

| Script | What it checks |
|--------|----------------|
| `tools/audit-fpu-lifecycle.py` | Boot/trap/TCB/flag/restore patterns against upstream seL4 RISC-V FPU |
| `tools/audit-kernel-fpu.py` | Release disassembly: FP/SIMD stay in the current-arch FPU module |

`pack-image.py` runs `audit-kernel-fpu.py` for the kernel ELF of the selected arch.

`audit-fpu-lifecycle.py` is not wired into any gate and its patterns still
describe the pre-`ktypes` function signatures (`*mut Tcb`, `unsafe fn`), so it
reports failures that are naming, not behaviour. `audit-kernel-fpu.py` is the
one that runs.

There is no local IPC fastpath, so there is no `nativeThreadUsingFPU`
fastpath guard to maintain.
