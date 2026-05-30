#!/usr/bin/env bash
#
# Build an xv6 user program, embed it into the xv6-host rootserver, pack that
# rootserver above the Rust kernel, and run the resulting image under QEMU.

set -euo pipefail

ROOT_DIR="$(cd "$(dirname "$0")/.." && pwd)"
TIMEOUT="${TIMEOUT:-30}"
SMP="${SMP:-2}"
XV6_ATTACH_FS_IMG="${XV6_ATTACH_FS_IMG:-1}"
XV6_BUILD_FS_IMG="${XV6_BUILD_FS_IMG:-1}"
XV6_FS_IMG="${XV6_FS_IMG:-${ROOT_DIR}/target/xv6compat/fs.img}"
VERBOSE=0

log() { printf '[run-xv6-user] %s\n' "$*" >&2; }
die() { log "ERROR: $*"; exit 1; }

usage() {
    cat >&2 <<'EOF'
usage: tools/run-xv6-user.sh [--verbose|-v] [--stdin TEXT | --stdin-file PATH] PROGRAM [ARG...]

Examples:
  tools/run-xv6-user.sh echo hello from xv6
  tools/run-xv6-user.sh --stdin 'echo hi
' sh
  tools/run-xv6-user.sh --stdin-file script.sh sh
  TIMEOUT=60 tools/run-xv6-user.sh sh

Environment:
  XV6_ATTACH_FS_IMG=0  boot without attaching xv6 fs.img as virtio-blk
  XV6_BUILD_FS_IMG=0   attach existing XV6_FS_IMG without rebuilding it
  XV6_FS_IMG=PATH      fs image path, default target/xv6compat/fs.img
EOF
}

while [[ $# -gt 0 ]]; do
    case "$1" in
        --verbose|-v)
            VERBOSE=1
            shift
            ;;
        --stdin)
            [[ $# -ge 2 ]] || die "--stdin requires text"
            export XV6_CONSOLE_INPUT="$2"
            shift 2
            ;;
        --stdin-file)
            [[ $# -ge 2 ]] || die "--stdin-file requires a path"
            [[ -f "$2" ]] || die "stdin file not found: $2"
            XV6_CONSOLE_INPUT="$(cat "$2"; printf _xv6_host_eof_marker_)"
            XV6_CONSOLE_INPUT="${XV6_CONSOLE_INPUT%_xv6_host_eof_marker_}"
            export XV6_CONSOLE_INPUT
            shift 2
            ;;
        --help|-h)
            usage
            exit 0
            ;;
        *)
            break
            ;;
    esac
done

[[ $# -ge 1 ]] || {
    usage
    exit 2
}

PROGRAM="${1#_}"

if ! command -v qemu-system-riscv64 >/dev/null 2>&1; then
    die "qemu-system-riscv64 not on PATH; run via nix develop"
fi

ROOTSERVER_ELF="$("${ROOT_DIR}/tools/build-xv6-user-rootserver.sh" "$@")"
PACKED_IMAGE="${ROOT_DIR}/images/xv6-${PROGRAM}-image-riscv-qemu-riscv-virt"
LOG_FILE="${LOG_FILE:-${ROOT_DIR}/target/xv6-${PROGRAM}-last-run.log}"

if [[ "${XV6_ATTACH_FS_IMG}" == "1" && "${XV6_BUILD_FS_IMG}" == "1" ]]; then
    XV6_FS_IMG="$(XV6_FS_IMG="${XV6_FS_IMG}" "${ROOT_DIR}/tools/build-xv6-fs-img.sh")"
fi
if [[ "${XV6_ATTACH_FS_IMG}" == "1" && ! -f "${XV6_FS_IMG}" ]]; then
    die "XV6_FS_IMG not found: ${XV6_FS_IMG}"
fi

log "packing image"
ROOTSERVER_ELF="${ROOTSERVER_ELF}" OUT_IMAGE="${PACKED_IMAGE}" "${ROOT_DIR}/tools/pack-image.sh"

mkdir -p "$(dirname "${LOG_FILE}")"
: >"${LOG_FILE}"

qemu_cmd=(
    qemu-system-riscv64
    -machine virt
    -cpu rv64
    -smp "${SMP}"
    -m 3072
    -nographic
    -bios none
    -kernel "${PACKED_IMAGE}"
)
if [[ "${XV6_ATTACH_FS_IMG}" == "1" ]]; then
    qemu_cmd+=(
        -global virtio-mmio.force-legacy=false
        -drive "file=${XV6_FS_IMG},if=none,format=raw,id=xv6fs"
        -device "virtio-blk-device,drive=xv6fs,bus=virtio-mmio-bus.0"
    )
fi

log "booting ${PROGRAM}; log: ${LOG_FILE}"
if [[ "${VERBOSE}" -eq 1 ]]; then
    "${qemu_cmd[@]}" 2>&1 | tee "${LOG_FILE}" &
else
    "${qemu_cmd[@]}" >"${LOG_FILE}" 2>&1 &
fi
qemu_pid=$!

cleanup() {
    if kill -0 "${qemu_pid}" 2>/dev/null; then
        kill -TERM "${qemu_pid}" 2>/dev/null || true
        sleep 0.2
        kill -KILL "${qemu_pid}" 2>/dev/null || true
    fi
}
trap cleanup EXIT INT TERM

deadline=$(( $(date +%s) + TIMEOUT ))
status=2
root_exit_re='xv6-host: exit\([^)]*\) pid=1([^0-9]|$)'
root_exit_ok_re='xv6-host: exit\(0\) pid=1([^0-9]|$)'
while [[ $(date +%s) -lt ${deadline} ]]; do
    if grep -qE "${root_exit_ok_re}" "${LOG_FILE}" 2>/dev/null; then
        status=0
        break
    fi
    if grep -qE "${root_exit_re}" "${LOG_FILE}" 2>/dev/null; then
        status=1
        break
    fi
    if grep -qE '\*\*\* KERNEL PANIC|kernel-mode trap|user fault:' "${LOG_FILE}" 2>/dev/null; then
        status=1
        break
    fi
    if ! kill -0 "${qemu_pid}" 2>/dev/null; then
        wait "${qemu_pid}" 2>/dev/null || true
        if grep -qE "${root_exit_ok_re}" "${LOG_FILE}" 2>/dev/null; then
            status=0
        else
            status=1
        fi
        break
    fi
    sleep 0.2
done

cleanup
trap - EXIT INT TERM

# QEMU can terminate or be killed by cleanup before the monitor loop observes
# the final root exit line. Treat the fully flushed log as authoritative.
if grep -qE "${root_exit_ok_re}" "${LOG_FILE}" 2>/dev/null; then
    status=0
fi

case "${status}" in
    0)
        exit_line="$(grep -E "${root_exit_ok_re}" "${LOG_FILE}" | tail -1)"
        echo "PASS: ${exit_line}"
        echo "      log: ${LOG_FILE}"
        ;;
    1)
        echo "FAIL: xv6 user run aborted"
        echo "      log: ${LOG_FILE}"
        echo "      tail:"
        tail -30 "${LOG_FILE}" | sed 's/^/        /'
        ;;
    2)
        echo "TIMEOUT after ${TIMEOUT}s"
        echo "      log: ${LOG_FILE}"
        echo "      tail:"
        tail -30 "${LOG_FILE}" | sed 's/^/        /'
        ;;
esac

exit "${status}"
