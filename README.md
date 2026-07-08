# microkernel

`microkernel` is a Rust seL4-style kernel for RV64 `qemu-riscv-virt` and
LoongArch64 build targets, plus a user-space xv6 compatibility stack built on
top of a seL4-like capability ABI.

The current rel4 scope intentionally keeps the scheduler simpler than upstream
seL4 MCS: there are no `SchedContext`/`SchedControl` objects, dispatch is
cooperative round-robin, priority values are accepted only as compatibility
metadata, and all domain values collapse into one effective scheduling domain.
Repository user-space should not depend on priority scheduling, multiple
domains, or timer preemption for correctness.

The repository has two main parts:

- `kernel/`: the Rust kernel that boots through the upstream seL4 elfloader and
  implements the current rel4 seL4-style ABI subset.
- `userspace/`: no_std seL4 user libraries and servers, including an xv6 stack
  that runs xv6 user programs through user-space services rather than an
  in-kernel Unix compatibility layer.

Detailed status and historical notes live in:

- [docs/milestones/sel4.md](docs/milestones/sel4.md)
- [docs/milestones/xv6.md](docs/milestones/xv6.md)

## Repository Layout

```text
microkernel/
|-- kernel/                    # Rust seL4 kernel
|-- userspace/
|   |-- sel4-user/             # shared no_std seL4 user ABI wrappers
|   |-- xv6-abi/               # xv6 syscall/fs/disk protocol constants
|   |-- xv6-host/              # xv6 rootserver and syscall server
|   |-- vfs-server/            # Unix fd, path, pipe, and console VFS
|   |-- xv6fs-server/          # xv6 fs.img filesystem backend
|   |-- uart-server/           # user console UART server
|   `-- virtio-disk-server/    # virtio-blk device server
|-- third_party/
|   |-- sel4test/              # upstream seL4/sel4test submodule tree
|   `-- xv6-riscv/             # upstream xv6 tree for user programs/fs.img
|-- tools/                     # build, pack, QEMU, and test helpers
`-- docs/milestones/           # detailed project status
```

## Prerequisites

Use Nix with flakes enabled. The helper scripts assume the upstream seL4 tree
and build directory are available at:

```text
${SEL4_TREE_DIR:-./third_party/sel4test}
${SEL4_BUILD_DIR:-./third_party/sel4test/build-riscv64}
```

Initialize the upstream seL4/sel4test components with normal git submodules:

```sh
git submodule update --init --recursive third_party/sel4test
```

Enter the development environment with either:

```sh
direnv allow
```

or:

```sh
nix develop
```

The examples below can also be run as `nix develop --command ...` from outside
the shell.

## Run rel4 / seL4-Style Images

Build the Rust kernel explicitly:

```sh
cargo build --release --target riscv64gc-unknown-none-elf -p kernel
```

Pack the Rust kernel into an upstream seL4 elfloader image:

```sh
./tools/pack-image.py
```

Boot the packed image interactively under QEMU:

```sh
./tools/simulate.py
```

Run the packed image headlessly:

```sh
./tools/run-tests.py
```

Useful variants for tests that still match the current rel4 ABI subset:

```sh
SEL4TEST_REGEX='Test that there are tests' ./tools/pack-image.py
TIMEOUT=480 SMP=1 ./tools/run-tests.py
SMP=OFF NUM_NODES=1 ./tools/pack-image.py
```

The unmodified upstream `sel4test-driver` still assumes seL4's MCS
`SchedContext`/`SchedControl` ABI. After the rel4 no-MCS rollback, successful
image packing is useful, but upstream sel4test runs are not the default
correctness signal unless the selected slice avoids the removed scheduler
surface or the rootserver is adjusted.

## Run xv6 Programs

Build xv6's `fs.img`:

```sh
./tools/build-xv6-fs-img.py
```

Boot an interactive xv6 shell:

```sh
./tools/run-xv6-shell.py
```

Run individual xv6 user programs:

```sh
./tools/run-xv6-user.py echo hello from xv6
./tools/run-xv6-user.py forktest
./tools/run-xv6-user.py cat README
./tools/run-xv6-user.py ls .
```

Run the xv6 user test suite:

```sh
TIMEOUT=1200 ./tools/run-xv6-user.py usertests
```

Run a program with scripted console input:

```sh
./tools/run-xv6-user.py --stdin 'echo hi
exit
' sh
```

Run a timeout-expected workload:

```sh
TIMEOUT=90 ./tools/run-xv6-user.py --expect-timeout grind
```

## Common Checks

Format and type-check the Rust workspace:

```sh
cargo fmt --all --check
cargo check
```

Build the kernel package explicitly:

```sh
cargo build --release --target riscv64gc-unknown-none-elf -p kernel
cargo build --release --target loongarch64-unknown-none -p kernel
```

Check whether a LoongArch64-capable sel4tests tree is available:

```sh
./tools/check-loongarch-sel4tests.py
./tools/check-loongarch-sel4tests.py --manifest
SEL4_TREE_DIR=/path/to/loongarch64-sel4test \
  SEL4_BUILD_DIR=/path/to/loongarch64-sel4test/build-loongarch64 \
  ARCH=loongarch64 ./tools/pack-image.py
ARCH=loongarch64 ./tools/run-tests.py
```

See [docs/loongarch64-sel4tests.md](docs/loongarch64-sel4tests.md) for the
external seL4/libsel4/elfloader port pieces required by `ARCH=loongarch64`.

Current smoke path:

```sh
TIMEOUT=90 ARCH=riscv64 ./tools/run-xv6-user.py echo hello
```

Clean up a stuck QEMU test process if a run is interrupted:

```sh
pkill -TERM -f sel4test-driver-image-riscv-qemu-riscv-virt
pkill -TERM -f 'xv6-.*image-riscv-qemu-riscv-virt'
```
