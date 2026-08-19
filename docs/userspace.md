# Userspace

First-party user code is no_std. Every crate that talks to the kernel goes
through `userspace/sel4-user`. The xv6 stack is a set of servers, not an
in-kernel Unix layer.

Production builds are `riscv64gc-unknown-none-elf` only.
`sel4-user/src/arch/mod.rs`, `xv6-abi/src/platform/mod.rs`,
`xv6-host/src/arch/mod.rs`, and every server `build.rs` reject other targets.

## Crates

| Crate | Path | Role |
|-------|------|------|
| sel4-user | `userspace/sel4-user` | IPC wrappers, boot constants, `log` macros, `rt` |
| xv6-abi | `userspace/xv6-abi` | xv6 syscall numbers, VFS/FS/UART/disk opcodes, MMIO numbers |
| xv6-host | `userspace/xv6-host` | Rootserver: fault loop, process table, server spawn |
| vfs-server | `userspace/vfs-server` | fds, pipes, console routing, FS/UART client |
| xv6fs-server | `userspace/xv6fs-server` | xv6 on-disk FS over the disk server |
| virtio-disk-server | `userspace/virtio-disk-server` | virtio-blk |
| uart-server | `userspace/uart-server` | 16550 MMIO console |

## Process

`xv6-host/src/main.rs::run` allocates, creates a fault endpoint, embeds and
starts four servers, loads the initial payload ELF, then waits on the fault
endpoint.

Server spawn order and badges (`spawn_service_servers`):

1. uart-server
2. virtio-disk-server
3. xv6fs-server
4. vfs-server

Capability wiring is in the same function: VFS gets xv6fs + UART endpoints
and a reply endpoint back to the host; xv6fs gets the disk endpoint and a
completion notification.

`MAX_PROCS` is 64 (`xv6-host/src/consts.rs`).

## xv6 syscalls

`Xv6Syscall` in `xv6-abi/src/lib.rs` is 1–21. Dispatch is
`xv6-host/src/xv6.rs::handle_xv6_syscall`. Unknown numbers reply `-1`.

| # | Name | File |
|---|------|------|
| 1 | Fork | `process_syscalls.rs` |
| 2 | Exit | `process_syscalls.rs` |
| 3 | Wait | `process_syscalls.rs` |
| 4 | Pipe | `fs_syscalls.rs` |
| 5 | Read | `io_syscalls.rs` |
| 6 | Kill | `process_syscalls.rs` |
| 7 | Exec | `exec_syscalls.rs` |
| 8 | Fstat | `fs_syscalls.rs` |
| 9 | Chdir | `fs_syscalls.rs` |
| 10 | Dup | `fs_syscalls.rs` |
| 11 | GetPid | inline in `xv6.rs` |
| 12 | Sbrk | `memory_syscalls.rs` |
| 13 | Pause | `io_syscalls.rs` — one `sel4_yield()`, then 0 |
| 14 | Uptime | `TICKS` atomic in `xv6.rs` |
| 15 | Open | `fs_syscalls.rs` |
| 16 | Write | `io_syscalls.rs` |
| 17 | Mknod | `fs_syscalls.rs` |
| 18 | Unlink | `fs_syscalls.rs` |
| 19 | Link | `fs_syscalls.rs` |
| 20 | Mkdir | `fs_syscalls.rs` |
| 21 | Close | `fs_syscalls.rs` |

The host also handles VM faults (`memory_syscalls::handle_lazy_page_fault`)
and unhandled faults (`fault_kill`).

`sys_pause` does not sleep for `n` ticks. `pump_sleep_waiters` exists and
looks at `PROC_SLEEPING`, but `sys_pause` never enters that state.

## Exec and ELF

`exec_syscalls.rs` copies path/argv from the child, reads the file through
VFS (`vfs_read_exec_image`), resets mappings, then `child.rs::load_elf`.

`load_elf` accepts ELF64 little-endian `ET_EXEC` with machine 243 (RISC-V),
maps `PT_LOAD` segments, and sets brk to the aligned image end. The initial
root program is the payload ELF embedded by `xv6-host/build.rs`
(`include_bytes!` in `consts.rs`).

## IPC between servers

Opcodes are `xv6-abi` enums (`VfsOp`, `Xv6FsOp`, `UartOp`, `DiskRequestOp`)
and `Xv6Protocol` tags (`HostToVfs`, `VfsToXv6Fs`, `VfsToUart`, `FsToDisk`,
plus async variants).

Concurrency limits in the sources:

- Each server loop stages one reply cap (`reply_pending`).
- Host VFS client: `VFS_ASYNC_REQUEST_CAP = 16` (`xv6-host/src/vfs.rs`).
- Disk: `XV6_DISK_MAX_IN_FLIGHT = 2` (`xv6-abi`).
- Fork/exec/IO syscalls listed in `should_defer_vfs_syscall` wait while a
  VFS async request is in flight.

`sel4-user::rt` (`block_on`, `recv`, `reply_recv_with_reply`) is used by
vfs-server, xv6fs-server, and virtio-disk-server. uart-server and xv6-host
use a synchronous receive loop.

## Disk and console

On riscv64, `xv6-host/src/disk_transport/mmio.rs` maps
`VIRTIO_MMIO_FRAME_BASE` and issues `VIRTIO0_IRQ`. The device is
`virtio-disk-server/src/device/mmio.rs`. Platform numbers are in
`xv6-abi/src/platform/riscv64.rs`.

PCI sources exist (`disk_transport/pci.rs`, `device/pci.rs`) behind
`cfg(target_arch = "x86_64")`. There is no `xv6-abi` x86 platform module and
no x86 linker script, so those files do not build.

xv6 `read`/`write` on the console go host → vfs-server `console.rs` →
uart-server → 16550 MMIO at `XV6_UART_MMIO_VADDR`.

Rust `log` macros in every crate go through `sel4-user::UserLogger` →
`SYS_DEBUG_PUT_CHAR`. That is a different UART from the xv6 console. The
QEMU helpers attach a pci-serial chardev for the debug path.

## Not in the tree

- x86_64 user crates, linker scripts, or platform ABI
- xv6 syscalls beyond 1–21
- tick-accurate `pause(n)`
- `println!` runtime logging (only Cargo `println!` in `build.rs`)
