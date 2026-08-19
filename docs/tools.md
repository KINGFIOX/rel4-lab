# Tools

First-party helpers are `tools/*.py`. They default to `ARCH=riscv64`.
`tools/target_config.py` also knows `x86_64`.

`pack-image.py` and `run-ltp.py` drop `CARGO_TARGET_DIR` so Cargo writes
into the repository `target/` that the installer copies from.

## Pack and run

| Script | Behavior in code |
|--------|------------------|
| `pack-image.py` | Builds `-p kernel`, runs the kernel audit suite, injects the ELF into the sel4test elfloader, optional `ROOTSERVER_ELF`. Prints `image ready:`. |
| `run-hello.py` | x86_64 gate: builds kernel + `hello-rootserver`, runs the kernel audits, objcopies a Multiboot kernel, boots QEMU `-kernel`/`-initrd` `-serial stdio`. Success is `hello-rootserver: ok`. |
| `run-tests.py` | Boots an already packed image. Default `TIMEOUT=180`, `SMP=2`. Exit 0 on `Test suite passed.` (or a configured baseline fail banner). |
| `simulate.py` | Interactive QEMU. `MODE=image` if a packed image exists, else `standalone` kernel ELF. |
| `run-ltp.py` | Builds the ramfs cpio, linux-compat + vfs + uart, packs with `ROOTSERVER_ELF` and a 16-bit root CNode, boots QEMU without virtio-blk. Success is `ltp-wave1: ok`. |
| `build-linux-rootfs.py` | Cross-links wave-1 ET_EXEC programs (including LTP `uname01.c`) and writes a newc cpio. |

Upstream `sel4test-driver` still talks MCS `SchedContext` / `SchedControl`.
This kernel does not implement those objects, so a default `run-tests.py`
run is not a correctness signal for the current ABI.

## Audits

`pack-image.py::audit_rust_kernel` runs these on the kernel it just built:

| Script | Check |
|--------|--------|
| `audit-trap-layout.py` | RISC-V `trap.S` constants vs Rust `offset_of!`. The x86 trap path is wired (`arch/x86_64/trap.S`); this script still skips the x86 layout check. |
| `audit-user-context-abi.py` | `seL4_UserContext` word order |
| `audit-syscall-abi.py` | syscall numbers and object size bits vs `sel4-user` |
| `audit-smp-abi.py` | remote stall / IPI / scratch patterns |
| `audit-platform-abi.py` | UART / virtio MMIO constants vs `linux-abi` |
| `audit-vspace-abi.py` | page-table / ASID constants (x86 4-level) |
| `audit-kernel-elf.py` | kernel ELF entry and PT_LOAD layout |
| `audit-kernel-fpu.py` | FP instructions confined to the RV64 FPU module |

`audit-fpu-lifecycle.py` is a larger source-pattern suite for FPU ownership.
`audit-linux-compat-elf-abi.py` runs from `run-ltp.py` and checks RISC-V ELF
machine 243 for embedded uart/vfs payloads.

`kernel_arch_paths.py` and `tool_common.py` are shared helpers, not entry
points.
