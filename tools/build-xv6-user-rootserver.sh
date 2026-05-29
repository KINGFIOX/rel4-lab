#!/usr/bin/env bash
#
# Build one xv6 user program as a standalone seL4 rootserver payload.
#
# The program is still the xv6 user object linked with xv6's ulib/usys stubs.
# We add a tiny generated entry point that calls main(argc, argv), because xv6
# normally relies on exec() to lay out argv before jumping to user space.

set -euo pipefail

ROOT_DIR="$(cd "$(dirname "$0")/.." && pwd)"
XV6_DIR="${XV6_DIR:-${ROOT_DIR}/third_party/xv6-riscv}"
OUT_DIR="${OUT_DIR:-${ROOT_DIR}/target/xv6compat}"
XV6_USER_BASE="${XV6_USER_BASE:-0x10000000}"

log() { printf '[build-xv6-user] %s\n' "$*" >&2; }
die() { log "ERROR: $*"; exit 1; }

usage() {
    cat >&2 <<'EOF'
usage: tools/build-xv6-user-rootserver.sh PROGRAM [ARG...]

Examples:
  tools/build-xv6-user-rootserver.sh echo hello from xv6
  tools/build-xv6-user-rootserver.sh sh
EOF
}

infer_toolprefix() {
    for prefix in riscv64-none-elf- riscv64-unknown-elf- riscv64-elf- riscv64-linux-gnu- riscv64-unknown-linux-gnu-; do
        if command -v "${prefix}gcc" >/dev/null 2>&1; then
            printf '%s\n' "${prefix}"
            return 0
        fi
    done
    return 1
}

[[ $# -ge 1 ]] || {
    usage
    exit 2
}

PROGRAM="${1#_}"
shift

[[ -d "${XV6_DIR}" ]] || die "XV6_DIR not found: ${XV6_DIR}"
[[ -f "${XV6_DIR}/user/${PROGRAM}.c" ]] || die "xv6 user program not found: user/${PROGRAM}.c"

TOOLPREFIX="${TOOLPREFIX:-$(infer_toolprefix || true)}"
[[ -n "${TOOLPREFIX}" ]] || die "could not find a RISC-V ELF toolchain"

CC="${TOOLPREFIX}gcc"
LD="${TOOLPREFIX}ld"

mkdir -p "${OUT_DIR}"

log "building xv6 objects for ${PROGRAM}"
make -C "${XV6_DIR}" TOOLPREFIX="${TOOLPREFIX}" \
    "user/${PROGRAM}.o" user/ulib.o user/usys.o user/printf.o user/umalloc.o >/dev/null

ARGS_C="${OUT_DIR}/${PROGRAM}_argv.c"
ARGS_O="${OUT_DIR}/${PROGRAM}_argv.o"
OUT_ELF="${OUT_DIR}/_${PROGRAM}-rootserver"
LINKER_SCRIPT="${OUT_DIR}/user-${XV6_USER_BASE}.ld"

args=("${PROGRAM}" "$@")
argc="${#args[@]}"

{
    printf '#include "kernel/types.h"\n'
    printf '#include "user/user.h"\n\n'
    printf 'extern int main(int, char **);\n\n'
    for i in "${!args[@]}"; do
        escaped="$(printf '%s' "${args[$i]}" | perl -0pe 's/\\/\\\\/g; s/"/\\"/g; s/\n/\\n/g')"
        printf 'static char arg_%s[] = "%s";\n' "${i}" "${escaped}"
    done
    printf 'static char *compat_argv[] = {'
    for i in "${!args[@]}"; do
        printf ' arg_%s,' "${i}"
    done
    printf ' 0 };\n\n'
    printf 'void _xv6_compat_start(void) {\n'
    printf '  int r = main(%s, compat_argv);\n' "${argc}"
    printf '  exit(r);\n'
    printf '  for (;;) {}\n'
    printf '}\n'
} >"${ARGS_C}"

CFLAGS=(
    -Wall -Werror -Wno-unknown-attributes -O -fno-omit-frame-pointer
    -ggdb -gdwarf-2 -march=rv64gc -std=gnu99 -MD -mcmodel=medany
    -ffreestanding -fno-common -nostdlib -Wno-main
    -fno-builtin-strncpy -fno-builtin-strncmp -fno-builtin-strlen
    -fno-builtin-memset -fno-builtin-memmove -fno-builtin-memcmp
    -fno-builtin-bzero -fno-builtin-strchr -fno-builtin-malloc
    -fno-builtin-free -fno-builtin-memcpy -fno-builtin-printf
    -I"${XV6_DIR}" -fno-stack-protector -fno-pie -no-pie
)

"${CC}" "${CFLAGS[@]}" -c -o "${ARGS_O}" "${ARGS_C}"
perl -0pe "s/\\. = 0x0;/\\. = ${XV6_USER_BASE};/" \
    "${XV6_DIR}/user/user.ld" >"${LINKER_SCRIPT}"

log "linking ${OUT_ELF}"
"${LD}" -z max-page-size=4096 -T "${LINKER_SCRIPT}" -e _xv6_compat_start \
    -o "${OUT_ELF}" \
    "${ARGS_O}" \
    "${XV6_DIR}/user/${PROGRAM}.o" \
    "${XV6_DIR}/user/ulib.o" \
    "${XV6_DIR}/user/usys.o" \
    "${XV6_DIR}/user/printf.o" \
    "${XV6_DIR}/user/umalloc.o"

printf '%s\n' "${OUT_ELF}"
