# microkernel

`microkernel` is a Rust seL4-style kernel for RV64 `qemu-riscv-virt` and a
single-core x86_64 QEMU `pc` user bring-up, plus a user-space Linux RV64
compat stack (RISC-V only) built on a seL4-like capability ABI.

The current rel4 scope intentionally keeps the scheduler simpler than upstream
seL4 MCS: there are no `SchedContext`/`SchedControl` objects, dispatch is
unprioritised round-robin, priority values are accepted only as compatibility
metadata, and all domain values collapse into one effective scheduling domain.

There is no timeslice, quantum, or budget accounting, and no priority-driven
preemption. Kernel exit does, however, perform an unconditional round-robin
rotation for every trap cause, so a timer interrupt can involuntarily switch
away from a running thread whenever another runnable thread is queued on the
same core. The scheduler is therefore not cooperative. Repository user-space
should neither depend on priority scheduling, multiple domains, or preemption
for correctness, nor assume that a running thread executes uninterleaved.

The repository has two main parts:

- `kernel/`: the Rust kernel that boots through the upstream seL4 elfloader and
  implements the current rel4 seL4-style ABI subset.
- `userspace/`: no_std seL4 user libraries and servers, including a
  linux-compat stack that runs Linux RV64 user programs through user-space
  services rather than an in-kernel Unix compatibility layer.

Current-state notes, written from the source tree:

- [docs/kernel.md](docs/kernel.md)
- [docs/userspace.md](docs/userspace.md)
- [docs/fpu.md](docs/fpu.md)
- [docs/tools.md](docs/tools.md)

## Repository Layout

```text
microkernel/
|-- kernel/                    # Rust seL4 kernel
|-- userspace/
|   |-- sel4-user/             # shared no_std seL4 user ABI wrappers
|   |-- hello-rootserver/      # minimal x86/RV rootserver gate
|   |-- linux-abi/             # Linux RV64 syscall/VFS/UART protocol constants
|   |-- linux-compat/          # Linux rootserver and syscall server
|   |-- vfs-server/            # ramfs, pipe, and console VFS
|   |-- uart-server/           # user console UART server
|   `-- linux-rootfs/          # wave-1 ET_EXEC programs and LTP wrap
|-- third_party/
|   |-- sel4test/              # upstream seL4/sel4test submodule tree
|   `-- ltp/                   # upstream LTP sources for selected tests
|-- tools/                     # build, pack, QEMU, and test helpers
`-- docs/                      # current-state notes (from source)
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

x86_64 user bring-up (QEMU `pc`, single core):

```sh
TIMEOUT=60 ARCH=x86_64 SMP=OFF NUM_NODES=1 ./tools/run-hello.py
```

Success is the log line `hello-rootserver: ok`. This is not linux-compat
and is not `ARCH=x86_64 ./tools/run-tests.py`.

The unmodified upstream `sel4test-driver` still assumes seL4's MCS
`SchedContext`/`SchedControl` ABI. After the rel4 no-MCS rollback, successful
image packing is useful, but upstream sel4test runs are not the default
correctness signal unless the selected slice avoids the removed scheduler
surface or the rootserver is adjusted.

## Run linux-compat / LTP

Build the wave-1 ramfs cpio:

```sh
./tools/build-linux-rootfs.py
```

Run the RISC-V linux-compat gate:

```sh
TIMEOUT=180 ARCH=riscv64 ./tools/run-ltp.py
```

Success is the log line `ltp-wave1: ok`. The image has no virtio-blk
device. x86 remains `tools/run-hello.py`.

## Common Checks

Format and type-check the Rust workspace:

```sh
cargo fmt --all --check
cargo check
```

Build the kernel package explicitly:

```sh
cargo build --release --target riscv64gc-unknown-none-elf -p kernel
cargo build --release --target x86_64-unknown-none -p kernel
```

The x86_64 kernel target is compile-only in this phase: trap, timer, and
user-space are not wired yet.

Current smoke path:

```sh
TIMEOUT=180 ARCH=riscv64 ./tools/run-ltp.py
```

Clean up a stuck QEMU test process if a run is interrupted:

```sh
pkill -TERM -f sel4test-driver-image-riscv-qemu-riscv-virt
pkill -TERM -f 'linux-.*image-riscv-qemu-riscv-virt'
```
