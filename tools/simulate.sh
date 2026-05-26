#!/usr/bin/env bash
#
# Boot the kernel under QEMU `virt` (RV64).
#
# Two modes:
#
#   MODE=standalone   (default if no packed image exists)
#       Loads the raw kernel ELF directly with `-bios none -kernel`. This is
#       the M1 standalone path: our Rust kernel runs in M-mode and prints a
#       banner via the QEMU NS16550 UART.
#
#   MODE=image        (default if `images/sel4test-driver-image-...` exists)
#       Loads the packed image produced by `tools/pack-image.sh`. This is
#       the M2+ path: OpenSBI → elfloader → Rust kernel (S-mode) →
#       sel4test-driver in user mode.

set -euo pipefail

ROOT_DIR="$(cd "$(dirname "$0")/.." && pwd)"
KERNEL_ELF="${ROOT_DIR}/target/riscv64gc-unknown-none-elf/release/kernel"
PACKED_IMAGE="${ROOT_DIR}/images/sel4test-driver-image-riscv-qemu-riscv-virt"

MODE="${MODE:-}"
if [[ -z "${MODE}" ]]; then
    if [[ -f "${PACKED_IMAGE}" ]]; then
        MODE=image
    else
        MODE=standalone
    fi
fi

case "${MODE}" in
    standalone)
        if [[ ! -f "${KERNEL_ELF}" ]]; then
            echo "kernel ELF not found, building..." >&2
            (cd "${ROOT_DIR}" && cargo build --release)
        fi
        exec qemu-system-riscv64 \
            -machine virt \
            -cpu rv64 \
            -smp 1 \
            -m 128M \
            -nographic \
            -bios none \
            -kernel "${KERNEL_ELF}" \
            "$@"
        ;;
    image)
        if [[ ! -f "${PACKED_IMAGE}" ]]; then
            echo "packed image not found at ${PACKED_IMAGE}" >&2
            echo "run tools/pack-image.sh first" >&2
            exit 1
        fi
        exec qemu-system-riscv64 \
            -machine virt \
            -cpu rv64 \
            -smp 1 \
            -m 3072 \
            -nographic \
            -bios none \
            -kernel "${PACKED_IMAGE}" \
            "$@"
        ;;
    *)
        echo "unknown MODE=${MODE}" >&2
        exit 1
        ;;
esac
