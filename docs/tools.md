# Tools

First-party helpers are `tools/*.py`. They default to `ARCH=riscv64`.
`tools/target_config.py` also knows `x86_64`.

`pack-image.py` and `build-xv6-user-rootserver.py` drop `CARGO_TARGET_DIR`
so Cargo writes into the repository `target/` that the installer copies from.

## Pack and run

| Script | Behavior in code |
|--------|------------------|
| `pack-image.py` | Builds `-p kernel`, runs the kernel audit suite, injects the ELF into the sel4test elfloader, optional `ROOTSERVER_ELF`. Prints `image ready:`. |
| `run-tests.py` | Boots an already packed image. Default `TIMEOUT=180`, `SMP=2`. Exit 0 on `Test suite passed.` (or a configured baseline fail banner). |
| `simulate.py` | Interactive QEMU. `MODE=image` if a packed image exists, else `standalone` kernel ELF. |
| `run-xv6-user.py` | Builds payload + xv6-host, optional `fs.img`, packs with `ROOTSERVER_ELF`, boots. Success is `xv6-host: exit(0) pid=1`. |
| `run-xv6-shell.py` | Same path with `sh`. |
| `build-xv6-user-rootserver.py` | Cross-links an xv6 user program and cargo-builds the host/servers. |
| `build-xv6-fs-img.py` | Builds upstream xv6 `fs.img`. |

Upstream `sel4test-driver` still talks MCS `SchedContext` / `SchedControl`.
This kernel does not implement those objects, so a default `run-tests.py`
run is not a correctness signal for the current ABI.

## Audits

`pack-image.py::audit_rust_kernel` runs these on the kernel it just built:

| Script | Check |
|--------|--------|
| `audit-trap-layout.py` | `trap.S` constants vs Rust `offset_of!` |
| `audit-user-context-abi.py` | `seL4_UserContext` word order |
| `audit-syscall-abi.py` | syscall numbers and object size bits vs `sel4-user` |
| `audit-smp-abi.py` | remote stall / IPI / scratch patterns |
| `audit-platform-abi.py` | UART / virtio MMIO constants vs `xv6-abi` |
| `audit-vspace-abi.py` | page-table / ASID constants |
| `audit-kernel-elf.py` | kernel ELF entry and PT_LOAD layout |
| `audit-kernel-fpu.py` | FP instructions confined to the RV64 FPU module |

`audit-fpu-lifecycle.py` is a larger source-pattern suite for FPU ownership.
`audit-xv6-host-elf-abi.py` runs from `build-xv6-user-rootserver.py` and
checks RISC-V ELF machine 243 for embedded payloads.

`kernel_arch_paths.py`, `tool_common.py`, and `xv6-build-lock.py` are shared
helpers, not entry points.
