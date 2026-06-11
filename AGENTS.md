# Repository Guidelines

## Project Structure & Module Organization

This repository is a Rust workspace for an RV64 seL4-style microkernel and user-space xv6 compatibility stack. `kernel/` contains the kernel crate, with subsystem modules under `kernel/src/` such as `arch/`, `object/`, `api/`, `machine/`, and `abi/`. `userspace/` contains no_std user libraries and servers: `sel4-user`, `uart-server`, `virtio-disk-server`, `vfs-server`, `xv6fs-server`, `xv6-host`, and `xv6-abi`. Build, QEMU, packing, and test helpers live in `tools/`. Milestone notes are in `docs/milestones/`. Vendored external code is under `third_party/`; avoid changing it unless the task explicitly concerns upstream xv6 or seL4 lab material.

## Build, Test, and Development Commands

Use the Nix development shell before building:

```sh
nix develop
```

Key commands:

```sh
cargo fmt --all --check
cargo check
cargo build --release --target riscv64imac-unknown-none-elf -p kernel
./tools/pack-image.py
./tools/simulate.py
./tools/run-tests.py
./tools/build-xv6-fs-img.py
./tools/run-xv6-user.py forktest
TIMEOUT=1200 ./tools/run-xv6-user.py usertests
```

`pack-image.py` inserts the Rust kernel into the seL4 test image. `simulate.py` boots QEMU interactively. `run-tests.py` runs the packed seL4 tests headlessly.

## Coding Style & Naming Conventions

Follow standard Rust formatting with `cargo fmt`; the workspace uses the stable toolchain and the `riscv64imac-unknown-none-elf` target from `rust-toolchain.toml`. Use 4-space indentation, `snake_case` for functions and modules, `CamelCase` for types, and `SCREAMING_SNAKE_CASE` for constants. Keep unsafe, architecture-specific, and concurrency-sensitive code localized and documented where invariants are not obvious.

## Testing Guidelines

There is no conventional `tests/` tree; validation is primarily through workspace checks, seL4 test images, and xv6 user programs. Run `cargo check` for fast Rust validation, then choose the smallest QEMU test that covers the change. For seL4 regressions, use `SEL4TEST_REGEX='SCHED0003' ./tools/pack-image.py` followed by `./tools/run-tests.py`. For xv6 behavior, run targeted programs with `./tools/run-xv6-user.py <program>` before `usertests`.

## Commit & Pull Request Guidelines

Recent commits use short, imperative subjects, for example `Harden TCB and CSpace locking for SMP` and `Tighten SMP handoff and ASID shootdown handling`. Keep subjects focused on the subsystem and behavior changed. Pull requests should include a concise problem statement, the implementation approach, commands run, and any remaining risk. Link related issues or milestone notes when applicable. Include logs or screenshots only when they clarify QEMU/test failures or interactive behavior.

## Security & Configuration Tips

Do not commit generated `target/`, `images/`, or temporary QEMU artifacts. The helper scripts assume `SEL4_TREE_DIR` and `SEL4_BUILD_DIR` point at the seL4 tree and build directory, defaulting to `third_party/sel4-lab/sel4test` and its `build-riscv64` directory.
