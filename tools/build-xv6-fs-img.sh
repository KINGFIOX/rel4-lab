#!/usr/bin/env bash
#
# Build xv6's native fs.img with the same no-F/D RISC-V user-program flags used
# by the xv6-host payload path, then copy it under target/xv6compat.

set -euo pipefail

ROOT_DIR="$(cd "$(dirname "$0")/.." && pwd)"
XV6_DIR="${XV6_DIR:-${ROOT_DIR}/third_party/xv6-riscv}"
OUT_DIR="${OUT_DIR:-${ROOT_DIR}/target/xv6compat}"
XV6_FS_IMG="${XV6_FS_IMG:-${OUT_DIR}/fs.img}"
XV6_USER_MARCH="${XV6_USER_MARCH:-rv64imac}"
XV6_USER_MABI="${XV6_USER_MABI:-lp64}"

log() { printf '[build-xv6-fs-img] %s\n' "$*" >&2; }
die() { log "ERROR: $*"; exit 1; }

infer_toolprefix() {
    for prefix in riscv64-none-elf- riscv64-unknown-elf- riscv64-elf- riscv64-linux-gnu- riscv64-unknown-linux-gnu-; do
        if command -v "${prefix}gcc" >/dev/null 2>&1; then
            printf '%s\n' "${prefix}"
            return 0
        fi
    done
    return 1
}

[[ -d "${XV6_DIR}" ]] || die "XV6_DIR not found: ${XV6_DIR}"

TOOLPREFIX="${TOOLPREFIX:-$(infer_toolprefix || true)}"
[[ -n "${TOOLPREFIX}" ]] || die "could not find a RISC-V ELF toolchain"
HOST_CC="${HOST_CC:-$(command -v cc || command -v clang || command -v gcc || true)}"
[[ -n "${HOST_CC}" ]] || die "could not find a host C compiler for mkfs"

CFLAGS=(
    -Wall -Werror -Wno-unknown-attributes -O -fno-omit-frame-pointer
    -ggdb -gdwarf-2 -march="${XV6_USER_MARCH}" -mabi="${XV6_USER_MABI}"
    -std=gnu99 -MD -mcmodel=medany
    -ffreestanding -fno-common -nostdlib -Wno-main
    -fno-builtin-strncpy -fno-builtin-strncmp -fno-builtin-strlen
    -fno-builtin-memset -fno-builtin-memmove -fno-builtin-memcmp
    -fno-builtin-log -fno-builtin-bzero -fno-builtin-strchr
    -fno-builtin-exit -fno-builtin-malloc -fno-builtin-putc
    -fno-builtin-free -fno-builtin-memcpy
    -fno-builtin-printf -fno-builtin-fprintf -fno-builtin-vprintf
    -I. -fno-stack-protector -fno-pie -no-pie
)

log "building host mkfs"
"${HOST_CC}" -Wno-unknown-attributes -I"${XV6_DIR}" -o "${XV6_DIR}/mkfs/mkfs" "${XV6_DIR}/mkfs/mkfs.c"

log "building xv6 fs.img"
make -C "${XV6_DIR}" TOOLPREFIX="${TOOLPREFIX}" CFLAGS="${CFLAGS[*]}" fs.img >/dev/null

mkdir -p "$(dirname "${XV6_FS_IMG}")"
install -m 0644 "${XV6_DIR}/fs.img" "${XV6_FS_IMG}"
log "fs image ready: ${XV6_FS_IMG}"
printf '%s\n' "${XV6_FS_IMG}"
