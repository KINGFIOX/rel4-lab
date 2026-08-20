# Tool Catalog

## Repository Entry Points

- `README.md`: high-level workflow and common commands. It is the first place to check for intended user-facing usage.
- `tools/tool_common.py`: shared helpers for paths, logging, command execution, build locks, RISC-V toolchain inference, Nix hardening cleanup, and QEMU log handling.

## Rust Checks

- `cargo fmt --all --check`: default formatting check.
- `cargo check`: default workspace type check.
- `cargo build --release --target riscv64gc-unknown-none-elf -p kernel`: explicit kernel build.

Run these directly from the current direnv-loaded shell when available.

## seL4 Tools

### `tools/pack-image.py`

Purpose: build the Rust kernel and pack it into the upstream sel4test elfloader image.

Key behavior:

- Builds `kernel` with `cargo build --release --target ${RUST_TARGET:-riscv64gc-unknown-none-elf} -p kernel`.
- Uses `${SEL4_BUILD_DIR:-third_party/sel4-lab/sel4test/build-riscv64}`.
- Infers `${SEL4_TREE_DIR}` from `SEL4_TREE_DIR`, `SEL4_ROOT`, or the default third-party path.
- Reconfigures upstream CMake only when cache/source/env overrides differ.
- Pins vendored sel4test CMake `MCS=OFF`. RISC-V also defaults `SMP=ON`, `NUM_NODES=2`. x86 defaults `SMP=OFF`, `NUM_NODES=1`, `Sel4testHaveTimer=ON`, and a narrow POSIX regex (CNode, IPC, `TIMER0001`).
- Honors `SEL4TEST_REGEX`, `SMP`, `NUM_NODES`, `SIMULATION`, `DOMAINS`, `ARM_HYP`, `RELEASE`, `VERIFICATION`, `BAMBOO`, and `QEMU_DTB`.
- Supports custom rootservers through `ROOTSERVER_ELF`.
- Writes `${OUT_IMAGE:-images/sel4test-driver-image-riscv-qemu-riscv-virt}`.

Use patterns:

- Focused seL4 test image: `SEL4TEST_REGEX='SCHED0003' tools/pack-image.py`.
- Single-core image: `SMP=OFF NUM_NODES=1 tools/pack-image.py`.
- Custom rootserver image: `ROOTSERVER_ELF=... OUT_IMAGE=... tools/pack-image.py`.

### `tools/run-tests.py`

Purpose: run the already packed sel4test image headlessly and classify result.

Key behavior:

- Requires the packed image for the selected `ARCH`; it does not pack.
- Defaults `TIMEOUT=180`, RISC-V `SMP=2`, x86 `SMP=1` with Multiboot `-kernel`/`-initrd`.
- Writes `${LOG_FILE:-target/sel4test-last-run.log}` and `${KERNEL_DEBUG_LOG_FILE:-target/sel4test-kernel-debug.log}`.
- Raises `RUST_LOG` to at least `info`.
- Treats `Test suite passed.` as success.
- Also treats the configured known baseline failure as success via `${SEL4TEST_EXPECTED_BASELINE:-121/125}`.
- Fails on `Test suite failed`, rootserver abort, or kernel panic.
- `--verbose` mirrors QEMU output live.

Use after `tools/pack-image.py`. The x86 gate is the default narrow unicore regex, not a full sel4test run.

### `tools/run-hello.py`

Purpose: smallest x86_64 single-core QEMU `pc` user bring-up gate. The other x86 gates are narrow sel4test and `ARCH=x86_64 run-ltp.py`.

Key behavior:

- Builds `kernel`, `sel4-user`, and `hello-rootserver` for `x86_64-unknown-none`.
- Runs the kernel ABI/ELF audits, then objcopies a Multiboot kernel.
- Boots QEMU `-kernel`/`-initrd`. Defaults `SMP=OFF` / `NUM_NODES=1`; `SMP=ON NUM_NODES=2` is the AP bring-up check.
- Success is the console line `hello-rootserver: ok`.

Use pattern:

- Default x86 gate: `TIMEOUT=60 ARCH=x86_64 SMP=OFF NUM_NODES=1 tools/run-hello.py`.
- SMP hello: `TIMEOUT=60 ARCH=x86_64 SMP=ON NUM_NODES=2 tools/run-hello.py`.

### `tools/simulate.py`

Purpose: interactive QEMU boot of either the packed image or standalone kernel ELF.

Key behavior:

- `MODE=image` boots the packed sel4test image.
- `MODE=standalone` boots the kernel ELF directly.
- Default mode is `image` when a packed image exists, otherwise `standalone`.
- Defaults `SMP=1`.
- Passes extra CLI args through to QEMU.

Use for interactive debugging, not automated pass/fail classification.

## linux-compat / LTP Tools

### `tools/run-ltp.py`

Purpose: build, pack, boot, and classify the wave-1 linux-compat LTP gate.

Key behavior:

- Builds the ramfs cpio through `tools/build-linux-rootfs.py`.
- Builds `uart-server`, `vfs-server`, and `linux-compat`.
- Packs a custom rootserver image through `tools/pack-image.py` with `KERNEL_ROOT_CNODE_SIZE_BITS=16`.
- Boots QEMU without virtio-blk.
- Defaults `TIMEOUT=180`, `SMP=1`, and `RUST_LOG>=info`.
- Writes default logs under `target/linux-*-last-run.log` and `target/linux-*-kernel-debug.log`.
- Success is the console line `ltp-wave1: ok`.
- Failure includes `ltp-wave1: fail`, kernel panic, or `linux-compat: fault kill`.
- `--verbose` mirrors QEMU output live.

Use patterns:

- Default RISC-V gate: `TIMEOUT=180 ARCH=riscv64 tools/run-ltp.py`.
- x86 linux-compat gate: `TIMEOUT=180 ARCH=x86_64 tools/run-ltp.py`.

### `tools/build-linux-rootfs.py`

Purpose: cross-link wave-1 `ET_EXEC` programs and pack a newc cpio for vfs-server ramfs.

Key behavior:

- Uses `userspace/linux-rootfs` plus `third_party/ltp/.../uname01.c`.
- Writes `${LINUX_ROOTFS_CPIO:-target/linux-compat/ARCH/rootfs.cpio}`.
- Infers `TOOLPREFIX` from available RISC-V toolchains.
- Clears Nix hardening flags for bare-metal RISC-V tools.
- Uses the shared `BuildLock`.

Usually run indirectly through `tools/run-ltp.py`.

## Shared Behavior

- `BuildLock` defaults to `target/linux-compat/.build.lock`, records the holder PID, and removes stale locks when the holder is dead.
- Nested tool calls respect `LINUX_BUILD_LOCK_HELD=1`.
- `bare_metal_tool_env()` clears Nix hardening/linker flags so RISC-V bare-metal tools do not emit irrelevant hardening warnings.
- `LoggedProcess` captures QEMU output, optionally mirrors it live, and can delay stdin injection until a log regex appears.
- Generated images and logs live under `images/` and `target/`; do not commit them.
