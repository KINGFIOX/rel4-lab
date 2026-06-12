# microkernel

`microkernel` is a Rust seL4-style kernel for RV64 `qemu-riscv-virt`, plus a
user-space xv6 compatibility stack built on top of that seL4 ABI.

The repository has two main parts:

- `kernel/`: the Rust kernel that boots through the upstream seL4 elfloader and
  can run the upstream `sel4test-driver` rootserver.
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
|   |-- sel4-lab/sel4test/     # upstream seL4/sel4test checkout
|   `-- xv6-riscv/             # upstream xv6 tree for user programs/fs.img
|-- tools/                     # build, pack, QEMU, and test helpers
`-- docs/milestones/           # detailed project status
```

## Prerequisites

Use Nix with flakes enabled. The helper scripts assume the upstream seL4 tree
and build directory are available at:

```text
${SEL4_TREE_DIR:-./third_party/sel4-lab/sel4test}
${SEL4_BUILD_DIR:-./third_party/sel4-lab/sel4test/build-riscv64}
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

## Run seL4

Build the Rust kernel explicitly:

```sh
cargo build --release --target riscv64gc-unknown-none-elf -p kernel
```

Pack the Rust kernel into the upstream `sel4test` image:

```sh
./tools/pack-image.py
```

Boot the packed image interactively under QEMU:

```sh
./tools/simulate.py
```

Run the packed `sel4test` image headlessly:

```sh
./tools/run-tests.py
```

Useful variants:

```sh
SEL4TEST_REGEX='SCHED0003' ./tools/pack-image.py
TIMEOUT=480 SMP=1 ./tools/run-tests.py
SMP=OFF NUM_NODES=1 ./tools/pack-image.py
```

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
```

Clean up a stuck QEMU test process if a run is interrupted:

```sh
pkill -TERM -f sel4test-driver-image-riscv-qemu-riscv-virt
pkill -TERM -f 'xv6-.*image-riscv-qemu-riscv-virt'
```
