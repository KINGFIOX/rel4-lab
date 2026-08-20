# microkernel

`microkernel` is a Rust seL4-style kernel for RV64 `qemu-riscv-virt` and
x86_64 QEMU `pc`, plus a user-space Linux compatibility stack built on a
seL4-like capability ABI. x86 gates are hello-rootserver, a narrow unicore
sel4test slice, and `ARCH=x86_64 ./tools/run-ltp.py`.

Scheduling is unprioritised timeslice round-robin: a still-runnable thread
keeps the CPU until its timeslice expires or it Yields, blocks, or is
suspended. Ordinary traps do not rotate the runqueue. There is no
priority-driven preemption. Repository
user-space should neither depend on priority scheduling or multiple
domains for correctness, nor assume that a CPU-bound thread will be
switched away before its slice ends.

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
|   |-- linux-abi/             # Linux RV64/x86_64 syscall/VFS/UART protocol constants
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

x86_64 gates (QEMU `pc`):

```sh
TIMEOUT=60 ARCH=x86_64 SMP=OFF NUM_NODES=1 ./tools/run-hello.py
ARCH=x86_64 SMP=OFF NUM_NODES=1 ./tools/pack-image.py
TIMEOUT=180 ARCH=x86_64 ./tools/run-tests.py
TIMEOUT=180 ARCH=x86_64 ./tools/run-ltp.py
```

Success lines are `hello-rootserver: ok`, `Test suite passed.`, and
`ltp-wave1: ok`. x86 pack defaults to `SMP=OFF`, `NUM_NODES=1`,
`Sel4testHaveTimer=ON`, and a narrow POSIX regex covering CNode, IPC, and
`TIMER0001`.

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
device. The same script accepts `ARCH=x86_64`.

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

The x86_64 kernel boots on QEMU `pc` as a Multiboot image with
`syscall`/`sysret`, lazy FPU, IOAPIC IRQs, and x2APIC IPI. Gates are
hello-rootserver, a narrow unicore sel4test slice, and
`ARCH=x86_64 ./tools/run-ltp.py`. `NUM_NODES=2` hello checks AP bring-up.

Current smoke paths:

```sh
TIMEOUT=180 ARCH=riscv64 ./tools/run-ltp.py
TIMEOUT=60 ARCH=x86_64 SMP=OFF NUM_NODES=1 ./tools/run-hello.py
TIMEOUT=60 ARCH=x86_64 SMP=ON NUM_NODES=2 ./tools/run-hello.py
TIMEOUT=180 ARCH=x86_64 ./tools/run-ltp.py
```

Clean up a stuck QEMU test process if a run is interrupted:

```sh
pkill -TERM -f sel4test-driver-image-riscv-qemu-riscv-virt
pkill -TERM -f 'linux-.*image-riscv-qemu-riscv-virt'
```
