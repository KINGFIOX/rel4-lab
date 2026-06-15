# LoongArch64 sel4tests

The in-tree Rust kernel has a LoongArch64 build target and repository audits for
the LoongArch64 trap, VSpace, platform, SMP, syscall, and FPU interfaces.
Running upstream `sel4test-driver` on LoongArch64 also needs a LoongArch-capable
upstream seL4/sel4test tree.

The vendored tree under `third_party/sel4-lab/sel4test` currently provides the
RISC-V seL4 test image path. It does not currently include the upstream
LoongArch64 seL4 kernel, libsel4, and elfloader port pieces that CMake needs to
build a LoongArch64 sel4test image.

## Check Availability

Use the repository checker before attempting to pack a LoongArch64 image:

```sh
./tools/check-loongarch-sel4tests.py
```

To make missing pieces a CI failure:

```sh
./tools/check-loongarch-sel4tests.py --strict
```

To print the required external-tree manifest:

```sh
./tools/check-loongarch-sel4tests.py --manifest
```

## Required External Tree

Provide an external seL4/sel4test checkout with these LoongArch64 port pieces:

```text
kernel/src/arch/loongarch or kernel/src/arch/loongarch64
kernel/libsel4/sel4_arch_include/loongarch64
tools/seL4/elfloader-tool/src/arch-loongarch or tools/seL4/elfloader-tool/src/arch-loongarch64
tools/seL4/elfloader-tool/include/arch-loongarch or tools/seL4/elfloader-tool/include/arch-loongarch64
```

That tree also needs the normal `projects/sel4test` source directory and
`init-build.sh` entry point.

## Pack And Run

After providing such a tree, connect it to the repository tools:

```sh
SEL4_TREE_DIR=/path/to/loongarch64-sel4test \
  SEL4_BUILD_DIR=/path/to/loongarch64-sel4test/build-loongarch64 \
  ARCH=loongarch64 ./tools/check-loongarch-sel4tests.py --strict

SEL4_TREE_DIR=/path/to/loongarch64-sel4test \
  SEL4_BUILD_DIR=/path/to/loongarch64-sel4test/build-loongarch64 \
  ARCH=loongarch64 ./tools/pack-image.py

ARCH=loongarch64 ./tools/run-tests.py
```

`pack-image.py` builds and audits the Rust LoongArch64 kernel before touching
the seL4 tree. If the external seL4 tree is incomplete, it stops before CMake
configuration and reports the missing upstream port paths.
