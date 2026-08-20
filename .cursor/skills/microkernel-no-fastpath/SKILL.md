---
name: microkernel-no-fastpath
description: This kernel has one IPC path and no fastpath/slowpath split. Use when reviewing or changing trap entry, syscall dispatch, IPC, reply, FPU restore, or docs so seL4-style fastpath_call, fastpath_reply_recv, KernelFastpath, and fastpath/slowpath names stay out of first-party sources.
---

# Microkernel Has No Fastpath

没有 IPC 快路径
功能靠慢路径也能做，只是更慢。不是对错问题。

This kernel has one IPC path. Do not add a fastpath, a slowpath, or names
that split the two. For simplicity, first-party sources must not set up
fastpath and slowpath at all.

Describe `Call` / `Send` / `Recv` / `Reply` / `ReplyRecv` as the IPC path,
not as a slow path and not as a subset of seL4 fastpath.

## Do not add

- `fastpath_call`, `fastpath_reply_recv`, `fastpath_signal`, trap stubs
  such as `c_handle_fastpath_*`, or a `fastpath` module.
- `KernelFastpath`, `CONFIG_FASTPATH`, or comments that mention fastpath /
  slowpath / `nativeThreadUsingFPU` fastpath guards.
- Stubs whose only job is to reject fastpath or to say "no fastpath yet".
- Treating a missing fastpath as a seL4 alignment gap to fill.

Vendored `third_party/sel4test` may still mention fastpath. That is not a
reason to add one here.

## Preserve

- Ordinary endpoint, notification, and reply-cap IPC in `api/ipc.rs` and
  `api/syscall.rs`.
- Trap dispatch that sends every IPC syscall through those handlers, then
  `kernel_exit`.
- Architecture-neutral IPC for `riscv64` and `x86_64`.

## Validation

- Rust-only edits: `cargo fmt --all --check`, then `cargo check`.
