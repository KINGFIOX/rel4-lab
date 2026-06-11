# xv6 Compatibility Status

## Current Architecture

The old in-kernel xv6 compatibility shim has been retired. The active design is
a set of user-space seL4 servers:

```text
xv6 user program
  -> UnknownSyscall fault IPC
  -> xv6-host
  -> vfs-server
  -> xv6fs-server
  -> virtio-disk-server
  -> Rust seL4 kernel
  -> OpenSBI / QEMU virtio-blk
```

Console and logging are deliberately split from that storage path:

```text
VFS console file ops -> uart-server -> QEMU serial0
kernel/server logs   -> SYS_DEBUG_PUT_CHAR/debug UART -> QEMU pci-serial log
```

Shared no_std user-space ABI wrappers live in `userspace/sel4-user`, and common
xv6 syscall/fs/disk protocol constants live in `userspace/xv6-abi`.

## Current xv6 Rootserver Level

`xv6-host` is a no_std Rust 2024 rootserver. It creates child TCB/CNode/VSpace
instances with fault endpoints, receives xv6 positive syscalls as
`UnknownSyscall` fault IPC, and implements handlers for xv6 syscall numbers
1 through 21.

Currently functional areas:

- Process lifecycle: TCB/VSpace-backed `fork`, normal `exit`, fault kill,
  `kill(pid)`, zombie state, reparenting, blocking `wait`, and atomic
  host-side PID/tick allocation, typed process-table ownership, allocator-owned
  recycled slot state, sparse eager reservation accounting, and VFS
  request/deferred-reply scalar state without `static mut`.
- Exec: `exec()` loads real xv6 user ELF bytes from `fs.img` through
  `vfs-server -> xv6fs-server -> virtio-disk-server`; the host-side embedded
  exec catalog is no longer the normal source of binaries.
- Host/VFS split: `Child` owns the user fd table and cwd path string; VFS does
  not keep a process table and only receives host-supplied file handles and
  absolute paths.
- File descriptors: shared VFS open-file offsets across `dup` and `fork`,
  close-on-exit from the host, `fstat`, and console/file/pipe read/write.
- Memory: `sbrk`, lazy user page allocation, guard behavior, fork resource
  preflight, and recycling of process-owned user frames/cap slots.
- VFS: syscall-level Unix filesystem dispatch, open-file refs/offsets,
  fixed-size in-memory pipe rings, console file operations through
  `uart-server`, and host-side VFS async request / exec-image scratch storage
  owned by typed state.
- xv6fs: mutable xv6 on-disk files and directories through `fs.img`, including
  open/create/truncate, read/write, link/unlink, mkdir/mknod, path validation,
  typed filesystem geometry state, inode refs owned by a typed cell, orphan
  recovery, xv6 redo-log recovery/commit, typed log/cache storage, and atomic
  transaction/read-gate scalar state.
- UART/logging: user-visible console I/O goes through the default UART0 owned by
  `uart-server`; Rust kernel output, `SYS_DEBUG_PUT_CHAR`, and server logs use a
  separate PCI serial debug UART through the Rust `log` facade.
- Async runtime: xv6 servers use `sel4-user::rt`, a no_std/no_alloc
  single-task cooperative runtime where `recv().await` and
  `reply_recv().await` perform the blocking seL4 syscall and resume after IPC,
  Notification, or IRQ delivery. The current VFS/xv6fs/disk model handles one
  request to completion before receiving the next service request.
- Blocking I/O: empty/full pipe and console would-block cases return
  `Xv6Status::WouldBlock` from VFS; the host saves the child reply cap and
  retries.
- Time/input: `uptime`, `pause(n)` backed by timer notifications, scripted
  console input, and runtime QEMU stdin polling through VFS console ops.
- Disk: independent `virtio-disk-server` with virtio-mmio setup, IRQ-driven
  completion, explicit request slots, shared data slots, flush support, and
  shared completion ring plus Notification delivery to xv6fs-server; device
  scalar state now uses atomics and the pending request table is owned by a
  typed cell rather than `static mut`, while xv6fs-side disk completion stash
  and scratch-slot tracking are owned by a typed runtime cell.
- Tooling: xv6 run/test tools force at least `RUST_LOG=info` for Rust
  kernel/server logs and clear Nix hardening flags for bare-metal tool builds so
  the old `-z relro ignored` / `-z now ignored` linker warnings do not mask real
  failures.

## Current Validation

Latest checked level after the custom runtime, UART/logging changes, VFS waiter
retry fix, standalone reparent cleanup, VM-fault ABI fix, and VFS `fstat`
size propagation:

```text
cargo fmt --all --check
cargo check -p sel4-user
TIMEOUT=120 ./tools/run-xv6-user.py usertests reparent
TIMEOUT=180 ./tools/run-xv6-user.py usertests reparent2
TIMEOUT=180 ./tools/run-xv6-user.py usertests sbrkmuch
TIMEOUT=1200 ./tools/run-xv6-user.py usertests
```

The full current-tip `usertests` result reached:

```text
ALL TESTS PASSED
xv6-host: exit(0) pid=1
```

Latest targeted frontier runs include:

```text
TIMEOUT=180 ./tools/run-xv6-user.py ls .
TIMEOUT=90 ./tools/run-xv6-user.py --expect-timeout grind
```

The `ls .` run exercised `fstat` size propagation through VFS/host stat
marshalling, including `README         2 2 2441` in the user-visible output.
The `grind` run reached the configured timeout without fatal xv6 output; its
debug log showed repeated fork/exec/exit and create/unlink churn. That is
timeout smoke coverage, not a claim that `grind` now completes.

Recorded targeted xv6 coverage before the final custom-runtime migration
included:

```text
./tools/run-xv6-user.py forktest
./tools/run-xv6-user.py cat README
./tools/run-xv6-user.py ls .
./tools/run-xv6-user.py wc README
./tools/run-xv6-user.py grep xv6 README
./tools/run-xv6-user.py stressfs
```

## Active Checkpoints

The early milestone table has been folded down. The temporary in-kernel shim,
read-only pseudo-filesystem, host-side embedded exec catalog, and the Embassy
runtime experiment are retired history, not active planning items.

| Area | Current checkpoint |
|------|--------------------|
| User-space architecture | xv6 compatibility is implemented as user-space seL4 servers rather than kernel-side compatibility code. |
| Process/syscall model | `xv6-host` handles xv6 syscalls 1..21 through `UnknownSyscall` fault IPC and owns process, address-space, fd-table, cwd, and child lifecycle state. |
| Executable source | `exec()` normally reads real xv6 ELF files from `fs.img`; the host-side embedded catalog is no longer the normal path. |
| VFS split | `vfs-server` owns syscall-level file, pipe, console, fd, and namespace dispatch while `xv6fs-server` owns the xv6 on-disk filesystem backend. |
| xv6fs backend | The filesystem server supports mutable files/directories, allocation, truncation, link/unlink, mkdir/mknod, path validation, inode refs, orphan recovery, and redo-log recovery/commit. |
| Disk server | `virtio-disk-server` is IRQ-driven, uses explicit request/shared slots, supports flush, and reports completions through a shared ring plus Notification. |
| Blocking I/O | Pipe and console would-block cases use saved reply caps and host-side retry instead of busy waiting. |
| Runtime/logging | Embassy has been replaced by the self-authored `sel4-user::rt` no_std/no_alloc runtime; user console I/O and debug/server logging are split across separate UART paths. |
| Current regression state | The full current-tip `TIMEOUT=1200 ./tools/run-xv6-user.py usertests` run passes with `ALL TESTS PASSED` and `xv6-host: exit(0) pid=1`; targeted `ls .` and timeout-mode `grind` smoke runs also pass. |

## Historical Notes

Earlier milestones from the temporary in-kernel xv6 shim, pre-fs-server
compatibility filesystem, and step-by-step module splitting have been removed
from the active tracker. They are still useful context when reading old commits,
but they should not drive current work.

The important historical result is the recorded full `usertests` pass in the
host-era compatibility model:

```text
ALL TESTS PASSED
xv6-host: exit(0) pid=1
```

Passing `usertests` does not mean the xv6 stack is a complete Unix server; it
means the current compatibility target is already useful for xv6 user programs.

## Remaining xv6 Work

The xv6 path is already useful for xv6 user programs and the recorded
`usertests` suite, but it is not a complete Unix server. Remaining work:

1. Decide whether the serial single-in-flight server model is enough for the
   long term or whether bounded concurrency should be reintroduced with explicit
   runtime primitives.
2. Expand device semantics beyond the current console/UART path and keep
   shaping the user-console protocol as the hardware model grows.
3. Improve resource reclamation toward full untyped/cap-object lifecycle
   recovery instead of mostly process-local frame/cap-slot recycling.
4. Decide whether to preserve upstream xv6 limits such as `MAXFILE` and
   `FSSIZE` or add an intentional larger-file/on-disk-format extension.
5. Extend and repeat longer-running workloads such as `grind` beyond the
   current 90-second timeout smoke coverage, and add targeted checks for
   fs/VFS/UART interactions under the current runtime.
6. Track any remaining bare-metal linker warnings separately from the old
   `-z relro` / `-z now` noise; if they matter, fix them in the corresponding
   toolchain or linker script rather than in the xv6 doc path itself.
