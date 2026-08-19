# Tools

First-party helpers are `tools/*.py`. They default to `ARCH=riscv64`.
`tools/target_config.py` also knows `x86_64`.

`pack-image.py` and `run-ltp.py` drop `CARGO_TARGET_DIR` so Cargo writes
into the repository `target/` that the installer copies from.

## Pack and run

| Script | Behavior in code |
|--------|------------------|
| `pack-image.py` | Builds `-p kernel`, runs the kernel audit suite, injects the ELF into the sel4test elfloader, optional `ROOTSERVER_ELF`. Prints `image ready:`. |
| `run-hello.py` | x86_64 gate: builds kernel + `hello-rootserver`, runs the kernel audits, objcopies a Multiboot kernel, boots QEMU `-kernel`/`-initrd` `-serial stdio`. Default `SMP=OFF NUM_NODES=1`. `SMP=ON NUM_NODES=2` is the AP bring-up check. Success is `hello-rootserver: ok`. |
| `run-tests.py` | Boots an already packed image. RISC-V default `SMP=2`; x86 default `SMP=1` and Multiboot `-kernel`/`-initrd`. Exit 0 on `Test suite passed.` |
| `simulate.py` | Interactive QEMU. `MODE=image` if a packed image exists, else `standalone` kernel ELF. x86 image mode splits kernel + initrd. |
| `run-ltp.py` | Builds the ramfs cpio, linux-compat + vfs + uart, packs with `ROOTSERVER_ELF` and a 16-bit root CNode. `ARCH=riscv64` or `ARCH=x86_64`. Success is `ltp-wave1: ok`. |
| `build-linux-rootfs.py` | Cross-links wave-1 ET_EXEC programs (including LTP `uname01.c`) and writes a newc cpio for the selected `ARCH`. |

Upstream `sel4test-driver` still talks MCS `SchedContext` / `SchedControl`.
This kernel does not implement those objects, so a default `run-tests.py`
run is not a correctness signal for the current ABI.

## Audits

`pack-image.py::audit_rust_kernel` runs these on the kernel it just built:

| Script | Check |
|--------|--------|
| `audit-trap-layout.py` | `trap.S` constants vs Rust `offset_of!` for the selected arch |
| `audit-user-context-abi.py` | `seL4_UserContext` word order |
| `audit-syscall-abi.py` | syscall numbers and object size bits vs `sel4-user` |
| `audit-smp-abi.py` | remote stall / IPI / scratch patterns |
| `audit-platform-abi.py` | UART / virtio MMIO constants vs `linux-abi` |
| `audit-vspace-abi.py` | page-table / ASID constants (x86 4-level) |
| `audit-kernel-elf.py` | kernel ELF entry and PT_LOAD layout |
| `audit-kernel-fpu.py` | FP instructions confined to the arch FPU module |

`audit-fpu-lifecycle.py` is a larger source-pattern suite for FPU ownership.
`audit-linux-compat-elf-abi.py` runs from `run-ltp.py` and checks ELF
machine 243 (RISC-V) or 62 (x86_64) for embedded uart/vfs payloads.

`kernel_arch_paths.py` and `tool_common.py` are shared helpers, not entry
points.
