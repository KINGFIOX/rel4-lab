# Userspace

First-party user code is no_std. Every crate that talks to the kernel goes
through `userspace/sel4-user`. Linux syscall semantics live in userspace
(`linux-compat` + `vfs-server`), not in the kernel.

`sel4-user` builds for `riscv64gc-unknown-none-elf` and `x86_64-unknown-none`.
The linux-compat stack does not: `linux-abi/src/platform/mod.rs`,
`linux-compat/src/arch/mod.rs`, and the server `build.rs` files still reject
non-RISC-V targets. `hello-rootserver` is the x86 user gate.

## Crates

| Crate | Path | Role |
|-------|------|------|
| sel4-user | `userspace/sel4-user` | IPC wrappers, boot constants, `log` macros, `rt` |
| hello-rootserver | `userspace/hello-rootserver` | Minimal rootserver: banner, Untyped_Retype Endpoint, NBRecv, `hello-rootserver: ok` |
| linux-abi | `userspace/linux-abi` | Linux RV64 syscall numbers, errno, VFS/UART opcodes, MMIO numbers |
| linux-compat | `userspace/linux-compat` | Rootserver: fault loop, process table, Linux syscall dispatch |
| vfs-server | `userspace/vfs-server` | ramfs, pipes, console routing, UART client |
| uart-server | `userspace/uart-server` | 16550 MMIO console |

Wave-1 user programs live under `userspace/linux-rootfs/` and are packed into
a newc cpio by `tools/build-linux-rootfs.py`.

## Process

`linux-compat/src/main.rs::run` allocates, creates a fault endpoint, embeds
and starts uart-server and vfs-server, initializes ramfs, creates pid 1,
loads `/ltp-wave1` from ramfs, then waits on the fault endpoint.

Server spawn order (`spawn_service_servers`):

1. uart-server
2. vfs-server

VFS gets the UART endpoint and a reply endpoint back to the host. There is
no disk server and no virtio-blk.

`MAX_PROCS` is 64 (`linux-compat/src/consts.rs`).

## Linux syscalls

Dispatch is `linux-compat/src/linux.rs::handle_linux_syscall`. The kernel
delivers `UnknownSyscall` fault IPC with Linux RV64 numbers in `a7` and
arguments in `a0`–`a5`. Unimplemented numbers reply `-ENOSYS`.

Wave-1 coverage includes `exit`/`exit_group`, `write`, `openat`/`close`,
`getpid`/`getppid`, `clone` (fork form), `wait4`/`waitid`, `read`,
`mkdirat`/`unlinkat`, `getcwd`/`chdir`, `uname`, `getuid`/`getgid`,
`clock_gettime`, `dup`/`dup3`, `pipe2`, plus musl-facing `brk`/`mmap`/
`mprotect`/`set_tid_address`/`set_robust_list`/`prctl`.

`pause`/`nanosleep` only `Yield`. They do not depend on timeslice
preemption.

## Exec and ELF

`exec_syscalls.rs` copies path/argv/envp from the child, reads the file
through VFS (`vfs_read_exec_image`), resets mappings, then
`child.rs::load_elf`.

`load_elf` accepts ELF64 little-endian `ET_EXEC` with machine 243 (RISC-V),
maps `PT_LOAD` segments, records PHDR auxv fields, and sets brk to the
aligned image end. The initial program is `/ltp-wave1` from ramfs, not an
embedded payload.

The Linux stack image includes argc, argv, envp, and auxv (`AT_PHDR`,
`AT_PAGESZ`, `AT_RANDOM`, `AT_NULL`, and related keys).

## IPC between servers

Opcodes are `linux-abi` enums (`VfsOp`, `UartOp`) and `IpcProtocol` tags
(`HostToVfs`, `HostToVfsAsync`, `VfsToUart`, `VfsToUartAsync`).

Concurrency limits in the sources:

- Each server loop stages one reply cap (`reply_pending`).
- Host VFS client: `VFS_ASYNC_REQUEST_CAP = 16` (`linux-compat/src/vfs.rs`).
- Fork/exec/IO syscalls listed in `should_defer_vfs_syscall` wait while a
  VFS async request is in flight.

`sel4-user::rt` is used by vfs-server. uart-server and linux-compat use a
synchronous receive loop.

## ramfs and console

vfs-server unpacks an embedded newc cpio into an in-memory ramfs at
`VfsOp::Init`. `/tmp` and `/dev/console` are created if missing. Console
`read`/`write` go host → vfs-server `console.rs` → uart-server → 16550
MMIO.

Rust `log` macros in every crate go through `sel4-user::UserLogger` →
`SYS_DEBUG_PUT_CHAR`. That is a different UART from the Linux console. The
QEMU helpers attach a pci-serial chardev for the debug path.

## Not in the tree

- x86_64 linux-compat
- complete LTP, networking, ptrace, `/proc`
- tick-accurate `nanosleep`
- `println!` runtime logging (only Cargo `println!` in `build.rs`)
