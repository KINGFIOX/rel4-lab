# Repository Guidelines

## Project Structure & Module Organization

This repository is a Rust workspace for an RV64 seL4-style microkernel with an x86_64 QEMU pc99 user bring-up and a user-space Linux RV64 compatibility stack (RISC-V). `kernel/` contains the kernel crate, with subsystem modules under `kernel/src/` such as `arch/`, `object/`, `api/`, `machine/`, and `abi/`. Architecture backends live under `kernel/src/arch/{riscv64,x86_64}` using a seL4-style compile-time `sel4_arch` / `machine` / `plat` split. `userspace/` contains no_std user libraries and servers: `sel4-user`, `uart-server`, `vfs-server`, `linux-compat`, and `linux-abi`. Build, QEMU, packing, and test helpers live in `tools/`. Current-state notes are in `docs/`. Vendored external code is under `third_party/`; avoid changing it unless the task explicitly concerns upstream LTP or seL4 lab material.

## Build, Test, and Development Commands

Use the Nix development shell before building:

```sh
nix develop
```

Key commands:

```sh
cargo fmt --all --check
cargo check
cargo build --release --target riscv64gc-unknown-none-elf -p kernel
cargo build --release --target x86_64-unknown-none -p kernel
./tools/pack-image.py
./tools/simulate.py
./tools/run-tests.py
./tools/build-linux-rootfs.py
TIMEOUT=180 ARCH=riscv64 ./tools/run-ltp.py
```

`pack-image.py` inserts the Rust kernel into the seL4 test image. `simulate.py` boots QEMU interactively. `run-tests.py` runs the packed seL4 tests headlessly.

## Coding Style & Naming Conventions

Follow standard Rust formatting with `cargo fmt`; the workspace uses the stable toolchain and the `riscv64gc-unknown-none-elf` target from `rust-toolchain.toml`. Use 4-space indentation, `snake_case` for functions and modules, `CamelCase` for types, and `SCREAMING_SNAKE_CASE` for constants. Keep unsafe, architecture-specific, and concurrency-sensitive code localized and documented where invariants are not obvious.

## Testing Guidelines

There is no conventional `tests/` tree; validation is primarily through workspace checks, seL4 test images, and linux-compat LTP programs. Run `cargo check` for fast Rust validation, then choose the smallest QEMU test that covers the change. For seL4 regressions, use `SEL4TEST_REGEX='SCHED0003' ./tools/pack-image.py` followed by `./tools/run-tests.py`. For Linux syscall behavior, use `TIMEOUT=180 ARCH=riscv64 ./tools/run-ltp.py`. The x86 gate is `TIMEOUT=60 ARCH=x86_64 SMP=OFF NUM_NODES=1 ./tools/run-hello.py`.

## Commit & Pull Request Guidelines

Recent commits use short, imperative subjects, for example `Harden TCB and CSpace locking for SMP` and `Tighten SMP handoff and ASID shootdown handling`. Keep subjects focused on the subsystem and behavior changed. Pull requests should include a concise problem statement, the implementation approach, commands run, and any remaining risk. Link related issues or milestone notes when applicable. Include logs or screenshots only when they clarify QEMU/test failures or interactive behavior.

## Security & Configuration Tips

Do not commit generated `target/`, `images/`, or temporary QEMU artifacts. The helper scripts assume `SEL4_TREE_DIR` and `SEL4_BUILD_DIR` point at the seL4 tree and build directory, defaulting to `third_party/sel4test` and its `build-riscv64` directory.
