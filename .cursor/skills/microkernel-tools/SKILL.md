---
name: microkernel-tools
description: Choose and run the repository helper tools for this Rust RV64 seL4/linux-compat microkernel workspace with an x86_64 QEMU pc user bring-up. Use when Codex needs a tool mindset for README.md and tools/, including formatting, cargo checks, kernel image packing, sel4test, QEMU simulation, linux-compat rootfs builds, LTP runs, logs, timeouts, and environment variables.
---

# Microkernel Tools

## Overview

Use the helper scripts as the project interface for build, packing, QEMU, seL4, and linux-compat validation. Prefer the smallest command that exercises the changed behavior, and load `references/tool-catalog.md` when exact script defaults or environment knobs matter.

## Tool Selection

1. For pure Rust edits:
   - Run `cargo fmt --all --check`.
   - Run `cargo check` or a narrower package check when the changed area is localized.
   - Build the kernel explicitly with `cargo build --release --target riscv64gc-unknown-none-elf -p kernel` when image or boot behavior depends on the kernel ELF.

2. For seL4 kernel validation:
   - Use `tools/pack-image.py` before any `sel4test` QEMU run. It builds the Rust kernel, configures/refreshes upstream sel4test if needed, injects the Rust kernel, and writes `images/sel4test-driver-image-riscv-qemu-riscv-virt`.
   - Use `SEL4TEST_REGEX='...' tools/pack-image.py` for focused test images.
   - Use `tools/run-tests.py` to run an already packed image headlessly and classify pass/fail/timeout.
   - Use `tools/simulate.py` for interactive QEMU boot debugging.

3. For linux-compat / LTP validation:
   - Use `TIMEOUT=180 ARCH=riscv64 tools/run-ltp.py` as the default RISC-V user gate. Success is `ltp-wave1: ok`.
   - Use `TIMEOUT=180 ARCH=x86_64 tools/run-ltp.py` as the x86 linux-compat gate.
   - Use `tools/build-linux-rootfs.py` when only the ramfs cpio needs refreshing.
   - The small x86 user gate remains `TIMEOUT=60 ARCH=x86_64 SMP=OFF NUM_NODES=1 tools/run-hello.py`.
   - The x86 sel4test gate is `ARCH=x86_64 SMP=OFF NUM_NODES=1 tools/pack-image.py` then `tools/run-tests.py`.

4. For environment handling:
   - Combine this with `$microkernel-dev-env`: run tools directly from the already-loaded direnv flake shell when available.
   - Do not wrap helper commands in `nix develop` unless the needed tool is missing and direct execution or `direnv exec . <command>` cannot work.

5. For logs and timeouts:
   - Prefer increasing `TIMEOUT` for expected long workloads instead of treating silence as success.
   - Inspect the printed `log:` and `kernel debug log:` paths after failures or timeouts.
   - Pass `--verbose` to `tools/run-tests.py` or `tools/run-ltp.py` when live QEMU output helps debugging.

## Reference

Read `references/tool-catalog.md` when you need:

- exact script behavior,
- default paths and output files,
- important environment variables,
- QEMU/log classification rules,
- linux-compat build-lock behavior.
