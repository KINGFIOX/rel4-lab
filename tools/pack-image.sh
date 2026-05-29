#!/usr/bin/env bash
#
# pack-image.sh — wrap our Rust kernel into a sel4test-driver-image so the
# existing QEMU simulate flow can boot it.
#
# Strategy
# --------
# The seL4 build at $SEL4_BUILD_DIR already produces:
#
#   elfloader/kernel.elf   (stripped copy of C kernel)
#   elfloader/rootserver   (copy of sel4test-driver)
#   kernel/kernel.dtb      (DTB for qemu-riscv-virt)
#
# These are packed into elfloader/archive.archive.o.cpio, which is incbin'd
# into archive.o, which is linked into the elfloader binary at
# images/sel4test-driver-image-riscv-qemu-riscv-virt.
#
# We replace `elfloader/kernel.elf` with a stripped copy of our Rust kernel,
# delete the downstream artifacts so ninja regenerates them, then run ninja
# targeting just the image. We touch our injected kernel.elf to a time newer
# than the original `kernel/kernel.elf` so ninja's CUSTOM_COMMAND that
# regenerates `elfloader/kernel.elf` is short-circuited (restat=1 on its
# rule).

set -euo pipefail

ROOT_DIR="$(cd "$(dirname "$0")/.." && pwd)"
SEL4_BUILD_DIR="${SEL4_BUILD_DIR:-/Users/wangfiox/sel4/sel4test/build-riscv64}"
RUST_TARGET_DIR="${ROOT_DIR}/target/riscv64gc-unknown-none-elf/release"
RUST_KERNEL_ELF="${RUST_TARGET_DIR}/kernel"
ROOTSERVER_ELF="${ROOTSERVER_ELF:-}"

OUT_DIR="${ROOT_DIR}/images"
OUT_IMAGE="${OUT_IMAGE:-${OUT_DIR}/sel4test-driver-image-riscv-qemu-riscv-virt}"

log() { printf '[pack-image] %s\n' "$*" >&2; }
die() { log "ERROR: $*"; exit 1; }

# 1. Build the Rust kernel.
log "building Rust kernel..."
(cd "${ROOT_DIR}" && cargo build --release)
[[ -f "${RUST_KERNEL_ELF}" ]] || die "Rust kernel ELF missing: ${RUST_KERNEL_ELF}"

# 2. Sanity-check the seL4 build dir exists.
[[ -d "${SEL4_BUILD_DIR}" ]] || die "SEL4_BUILD_DIR not found: ${SEL4_BUILD_DIR}"
[[ -f "${SEL4_BUILD_DIR}/kernel/kernel.dtb" ]] || \
    die "kernel/kernel.dtb missing — run upstream seL4 build first"
if [[ -n "${ROOTSERVER_ELF}" && ! -f "${ROOTSERVER_ELF}" ]]; then
    die "ROOTSERVER_ELF not found: ${ROOTSERVER_ELF}"
fi

# 3. Let upstream rebuild anything it thinks is stale before we inject the
# Rust kernel. This intentionally goes all the way to the stock image target:
# if CMake regeneration, rootserver relinks, or the C kernel strip rule need to
# run, they must happen before we replace elfloader/kernel.elf.
if [[ -z "${ROOTSERVER_ELF}" ]]; then
    # A previous custom-rootserver run leaves elfloader/rootserver newer than
    # the upstream copy. Remove it so the normal sel4test image can regenerate
    # from the real rootserver instead of accidentally reusing the custom one.
    rm -f "${SEL4_BUILD_DIR}/elfloader/rootserver"
fi
log "refreshing upstream image prerequisites..."
(cd "${SEL4_BUILD_DIR}" && ninja images/sel4test-driver-image-riscv-qemu-riscv-virt)
[[ -f "${SEL4_BUILD_DIR}/elfloader/rootserver" ]] || \
    die "elfloader/rootserver missing after upstream refresh"

# 4. Install our (stripped) kernel.elf where the elfloader cpio expects it.
log "installing Rust kernel into elfloader staging..."
STRIP="${STRIP:-riscv64-none-elf-strip}"
TMP_STRIPPED="$(mktemp -t rust-kernel.elf.XXXXXX)"
TMP_ROOTSERVER_STRIPPED=""
trap 'rm -f "${TMP_STRIPPED}" ${TMP_ROOTSERVER_STRIPPED:+"${TMP_ROOTSERVER_STRIPPED}"}' EXIT
"${STRIP}" "${RUST_KERNEL_ELF}" -o "${TMP_STRIPPED}"
install -m 0644 "${TMP_STRIPPED}" "${SEL4_BUILD_DIR}/elfloader/kernel.elf"

if [[ -n "${ROOTSERVER_ELF}" ]]; then
    log "installing custom rootserver: ${ROOTSERVER_ELF}"
    TMP_ROOTSERVER_STRIPPED="$(mktemp -t rootserver.elf.XXXXXX)"
    "${STRIP}" "${ROOTSERVER_ELF}" -o "${TMP_ROOTSERVER_STRIPPED}"
    rm -f "${SEL4_BUILD_DIR}/elfloader/rootserver"
    install -m 0644 "${TMP_ROOTSERVER_STRIPPED}" "${SEL4_BUILD_DIR}/elfloader/rootserver"
fi

# Bump mtime so ninja considers the C-kernel strip step up-to-date and doesn't
# re-overwrite the staging file. The sleep avoids same-second filesystems.
sleep 1
touch "${SEL4_BUILD_DIR}/elfloader/kernel.elf"
if [[ -n "${ROOTSERVER_ELF}" ]]; then
    touch "${SEL4_BUILD_DIR}/elfloader/rootserver"
fi

# 5. Wipe downstream so ninja regenerates them from our kernel.elf.
log "invalidating downstream artifacts..."
rm -f "${SEL4_BUILD_DIR}/elfloader/archive.archive.o.cpio"
rm -f "${SEL4_BUILD_DIR}/elfloader/archive.o"
rm -f "${SEL4_BUILD_DIR}/elfloader/elfloader"
rm -f "${SEL4_BUILD_DIR}/images/sel4test-driver-image-riscv-qemu-riscv-virt"

# 6. Re-run ninja for just the image. Use -j1 for stable error reporting.
log "running ninja to re-pack image..."
(cd "${SEL4_BUILD_DIR}" && ninja images/sel4test-driver-image-riscv-qemu-riscv-virt)

if ! cmp -s "${TMP_STRIPPED}" "${SEL4_BUILD_DIR}/elfloader/kernel.elf"; then
    die "elfloader/kernel.elf was overwritten after Rust injection"
fi
if [[ -n "${ROOTSERVER_ELF}" ]] && ! cmp -s "${TMP_ROOTSERVER_STRIPPED}" "${SEL4_BUILD_DIR}/elfloader/rootserver"; then
    die "elfloader/rootserver was overwritten after custom rootserver injection"
fi

# 7. Copy into our local images/ directory.
mkdir -p "${OUT_DIR}"
cp -f "${SEL4_BUILD_DIR}/images/sel4test-driver-image-riscv-qemu-riscv-virt" "${OUT_IMAGE}"
log "image ready: ${OUT_IMAGE}"
