#!/usr/bin/env bash
#
# Build one xv6 user program as the initial payload embedded in the xv6-host
# seL4 rootserver, plus a small exec() catalog of xv6 user ELFs.
#
# The xv6 program is still linked with xv6's ulib/usys stubs. A tiny generated
# entry point calls main(argc, argv), because xv6 normally relies on exec() to
# lay out argv. The resulting ELF is not booted directly; xv6-host loads it
# into a child TCB/VSpace and handles its positive syscalls via fault IPC.

set -euo pipefail

ROOT_DIR="$(cd "$(dirname "$0")/.." && pwd)"
XV6_DIR="${XV6_DIR:-${ROOT_DIR}/third_party/xv6-riscv}"
OUT_DIR="${OUT_DIR:-${ROOT_DIR}/target/xv6compat}"
XV6_USER_BASE="${XV6_USER_BASE:-0x10000000}"
XV6_EXEC_PROGRAMS="${XV6_EXEC_PROGRAMS:-init sh cat echo grep ls wc rm mkdir ln}"
XV6_USER_MARCH="${XV6_USER_MARCH:-rv64imac}"
XV6_USER_MABI="${XV6_USER_MABI:-lp64}"
RUST_TARGET="${RUST_TARGET:-riscv64imac-unknown-none-elf}"

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
    -I"${XV6_DIR}" -fno-stack-protector -fno-pie -no-pie
)

log "building xv6 objects for ${PROGRAM}"
read -r -a EXEC_PROGRAMS <<<"${XV6_EXEC_PROGRAMS}"
make_targets=("user/${PROGRAM}.o" user/ulib.o user/usys.o user/printf.o user/umalloc.o)
for prog in "${EXEC_PROGRAMS[@]}"; do
    [[ -f "${XV6_DIR}/user/${prog}.c" ]] || die "xv6 exec catalog program not found: user/${prog}.c"
    make_targets+=("user/${prog}.o")
done
make -B -C "${XV6_DIR}" TOOLPREFIX="${TOOLPREFIX}" CFLAGS="${CFLAGS[*]}" "${make_targets[@]}" >/dev/null

ARGS_C="${OUT_DIR}/${PROGRAM}_argv.c"
ARGS_O="${OUT_DIR}/${PROGRAM}_argv.o"
PAYLOAD_ELF="${OUT_DIR}/_${PROGRAM}-payload"
CATALOG_RS="${OUT_DIR}/exec_catalog-${PROGRAM}.rs"
HOST_ELF="${OUT_DIR}/xv6-host-${PROGRAM}-rootserver"
HOST_BUILD_ELF="${ROOT_DIR}/target/${RUST_TARGET}/release/xv6-host"
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

"${CC}" "${CFLAGS[@]}" -c -o "${ARGS_O}" "${ARGS_C}"
perl -0pe "s/\\. = 0x0;/\\. = ${XV6_USER_BASE};/" \
    "${XV6_DIR}/user/user.ld" >"${LINKER_SCRIPT}"

log "linking payload ${PAYLOAD_ELF}"
"${LD}" -z max-page-size=4096 -T "${LINKER_SCRIPT}" -e _xv6_compat_start \
    -o "${PAYLOAD_ELF}" \
    "${ARGS_O}" \
    "${XV6_DIR}/user/${PROGRAM}.o" \
    "${XV6_DIR}/user/ulib.o" \
    "${XV6_DIR}/user/usys.o" \
    "${XV6_DIR}/user/printf.o" \
    "${XV6_DIR}/user/umalloc.o"

cat >"${CATALOG_RS}" <<'EOF'
pub(crate) struct ExecImage {
    pub(crate) name: &'static [u8],
    pub(crate) elf: &'static [u8],
}

pub(crate) static EXEC_IMAGES: &[ExecImage] = &[
EOF

for prog in "${EXEC_PROGRAMS[@]}"; do
    exec_elf="${OUT_DIR}/_${prog}-exec"
    log "linking exec payload ${exec_elf}"
    "${LD}" -z max-page-size=4096 -T "${LINKER_SCRIPT}" -e start \
        -o "${exec_elf}" \
        "${XV6_DIR}/user/${prog}.o" \
        "${XV6_DIR}/user/ulib.o" \
        "${XV6_DIR}/user/usys.o" \
        "${XV6_DIR}/user/printf.o" \
        "${XV6_DIR}/user/umalloc.o"
    printf '    ExecImage { name: b"%s", elf: include_bytes!("%s") },\n' \
        "${prog}" "${exec_elf}" >>"${CATALOG_RS}"
done
printf '];\n' >>"${CATALOG_RS}"

log "building xv6-host rootserver ${HOST_ELF}"
XV6_PAYLOAD_ELF="${PAYLOAD_ELF}" XV6_EXEC_CATALOG_RS="${CATALOG_RS}" cargo build \
    --manifest-path "${ROOT_DIR}/Cargo.toml" \
    --release \
    --target "${RUST_TARGET}" \
    -p xv6-host
install -m 0644 "${HOST_BUILD_ELF}" "${HOST_ELF}"

printf '%s\n' "${HOST_ELF}"
