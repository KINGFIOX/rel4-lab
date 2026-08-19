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
- Defaults CMake to `MCS=ON`, `SMP=ON`, `NUM_NODES=2`, `SIMULATION=ON`, `PLATFORM=qemu-riscv-virt`, `KernelSel4Arch=riscv64`, and `LibSel4TestPrinterRegex=.*`.
- Honors `SEL4TEST_REGEX`, `SMP`, `NUM_NODES`, `MCS`, `SIMULATION`, `DOMAINS`, `ARM_HYP`, `RELEASE`, `VERIFICATION`, `BAMBOO`, and `QEMU_DTB`.
- Supports custom rootservers through `ROOTSERVER_ELF`.
- Writes `${OUT_IMAGE:-images/sel4test-driver-image-riscv-qemu-riscv-virt}`.

Use patterns:

- Focused seL4 test image: `SEL4TEST_REGEX='SCHED0003' tools/pack-image.py`.
- Single-core image: `SMP=OFF NUM_NODES=1 tools/pack-image.py`.
- Custom rootserver image: `ROOTSERVER_ELF=... OUT_IMAGE=... tools/pack-image.py`.

### `tools/run-tests.py`

Purpose: run the already packed sel4test image headlessly and classify result.

Key behavior:

- Requires `images/sel4test-driver-image-riscv-qemu-riscv-virt`; it does not pack.
- Defaults `TIMEOUT=180`, `SMP=2`.
- Writes `${LOG_FILE:-target/sel4test-last-run.log}` and `${KERNEL_DEBUG_LOG_FILE:-target/sel4test-kernel-debug.log}`.
- Raises `RUST_LOG` to at least `info`.
- Treats `Test suite passed.` as success.
- Also treats the configured known baseline failure as success via `${SEL4TEST_EXPECTED_BASELINE:-121/125}`.
- Fails on `Test suite failed`, rootserver abort, or kernel panic.
- `--verbose` mirrors QEMU output live.

Use after `tools/pack-image.py`.

### `tools/simulate.py`

Purpose: interactive QEMU boot of either the packed image or standalone kernel ELF.

Key behavior:

- `MODE=image` boots the packed sel4test image.
- `MODE=standalone` boots the kernel ELF directly.
- Default mode is `image` when a packed image exists, otherwise `standalone`.
- Defaults `SMP=1`.
- Passes extra CLI args through to QEMU.

Use for interactive debugging, not automated pass/fail classification.

## xv6 Tools

### `tools/run-xv6-user.py`

Purpose: build, pack, boot, and classify one xv6 user program under the seL4 server stack.

Key behavior:

- Builds a payload/rootserver through `tools/build-xv6-user-rootserver.py`.
- Builds or reuses an xv6 `fs.img`.
- Packs a custom rootserver image through `tools/pack-image.py`.
- Boots QEMU with virtio-blk attached unless disabled.
- Defaults `TIMEOUT=30`, `SMP=2`, and `RUST_LOG>=info`.
- Default image/log suffix is `${XV6_RUN_ID:-PROGRAM-PID}`.
- Writes default logs under `target/xv6-*-last-run.log` and `target/xv6-*-kernel-debug.log`.
- Success is root process `xv6-host: exit(0) pid=1`.
- Failure includes non-zero root exit, kernel panic, kernel-mode trap, or user fault.
- `--expect-timeout` or `XV6_EXPECT_TIMEOUT=1` can make timeout success if no fatal pattern appears.
- `--stdin` and `--stdin-file` set scripted console input through `XV6_CONSOLE_INPUT`.
- `--qemu-stdin` and `--qemu-stdin-file` feed QEMU stdin after `uart-server: init complete`.
- `--verbose` mirrors QEMU output live.

Important environment:

- `XV6_ATTACH_FS_IMG=0`: boot without virtio fs image.
- `XV6_BUILD_FS_IMG=0`: reuse an existing `XV6_FS_IMG`.
- `XV6_FS_IMG=PATH`: select fs image.
- `XV6_KEEP_RUN_FS_IMG=1`: keep the per-run copied fs image.
- `OUT_IMAGE=PATH`, `LOG_FILE=PATH`, `KERNEL_DEBUG_LOG_FILE=PATH`: override outputs.
- `XV6_EXPECT_TIMEOUT_FATAL_RE=REGEX`: customize timeout fatal patterns.

Use patterns:

- Smoke: `tools/run-xv6-user.py echo hello from xv6`.
- Targeted filesystem: `tools/run-xv6-user.py cat README`.
- Full suite: `TIMEOUT=1200 tools/run-xv6-user.py usertests`.
- Timeout workload: `TIMEOUT=90 tools/run-xv6-user.py --expect-timeout grind`.
- Scripted shell: `tools/run-xv6-user.py --stdin 'echo hi\nexit\n' sh`.

### `tools/build-xv6-user-rootserver.py`

Purpose: build an xv6 user payload and embed server ELF paths into `xv6-host`.

Key behavior:

- Requires `third_party/xv6-riscv/user/PROGRAM.c` unless `XV6_DIR` overrides the tree.
- Strips a leading `_` from program names.
- Uses `${OUT_DIR:-target/xv6compat}` and `${XV6_USER_BASE:-0x10000}`.
- Infers `TOOLPREFIX` from available RISC-V toolchains.
- Clears Nix hardening flags for bare-metal RISC-V tools.
- Builds `uart-server`, `vfs-server`, `xv6fs-server`, `virtio-disk-server`, then `xv6-host`.
- Prints the rootserver ELF path.
- Uses the shared `BuildLock`.

Usually run indirectly through `tools/run-xv6-user.py` or `tools/run-xv6-shell.py`.

### `tools/build-xv6-fs-img.py`

Purpose: build xv6's native `fs.img` and copy it to the compatibility output path.

Key behavior:

- Uses `${XV6_DIR:-third_party/xv6-riscv}`.
- Writes `${XV6_FS_IMG:-target/xv6compat/fs.img}`.
- Infers `TOOLPREFIX` and host C compiler.
- Clears Nix hardening flags for bare-metal tool builds.
- Uses the shared `BuildLock`.

Run directly when the filesystem image needs refreshing without booting QEMU.

### `tools/run-xv6-shell.py`

Purpose: build, pack, and boot an interactive xv6 shell.

Key behavior:

- Builds `sh` rootserver, optionally builds/copies `fs.img`, packs image, and runs QEMU with serial0 attached to the terminal.
- Requires a real TTY unless `--no-tty-check` is passed.
- Quit QEMU with `Ctrl-a x`.
- Defaults `SMP=2` and `XV6_RUN_ID=shell-PID`.
- Uses the same fs image knobs as `run-xv6-user.py`.

Use for interactive investigation, not automated regression.

### `tools/xv6-build-lock.py`

Compatibility wrapper for the old lock path. The real lock is `BuildLock` in `tool_common.py`.

## Shared Behavior

- `BuildLock` defaults to `target/xv6compat/.build.lock`, records the holder PID, and removes stale locks when the holder is dead.
- Nested tool calls respect `XV6_BUILD_LOCK_HELD=1`.
- `bare_metal_tool_env()` clears Nix hardening/linker flags so RISC-V bare-metal tools do not emit irrelevant hardening warnings.
- `LoggedProcess` captures QEMU output, optionally mirrors it live, and can delay stdin injection until a log regex appears.
- Generated images and logs live under `images/` and `target/`; do not commit them.
