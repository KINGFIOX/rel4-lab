#!/usr/bin/env bash
#
# Run the packed sel4test image under QEMU, watch for the upstream-defined
# pass / fail banners, and exit with a meaningful status. Suitable as a
# `pre-push` hook or CI step.
#
# Usage:
#   tools/run-tests.sh             # quiet — only prints summary + exit code
#   tools/run-tests.sh --verbose   # stream QEMU output as the test runs
#   TIMEOUT=300 tools/run-tests.sh # override hard timeout (seconds)
#   SMP=2 tools/run-tests.sh       # boot QEMU with two harts
#
# Exit codes:
#   0  - "Test suite passed." appears before EOF / timeout
#   1  - explicit failure banner ("Test suite failed", root server abort,
#        kernel panic, etc.)
#   2  - test run timed out without seeing either banner
#   3  - QEMU / image / build problem (couldn't even start)

set -uo pipefail

ROOT_DIR="$(cd "$(dirname "$0")/.." && pwd)"
PACKED_IMAGE="${ROOT_DIR}/images/sel4test-driver-image-riscv-qemu-riscv-virt"
LOG_FILE="${LOG_FILE:-${ROOT_DIR}/target/sel4test-last-run.log}"
TIMEOUT="${TIMEOUT:-180}"
SMP="${SMP:-1}"

VERBOSE=0
for arg in "$@"; do
    case "$arg" in
        --verbose|-v) VERBOSE=1 ;;
        *) echo "unknown arg: $arg" >&2; exit 3 ;;
    esac
done

if [[ ! -f "${PACKED_IMAGE}" ]]; then
    echo "packed image not found at ${PACKED_IMAGE}" >&2
    echo "run tools/pack-image.sh first" >&2
    exit 3
fi

if ! command -v qemu-system-riscv64 >/dev/null 2>&1; then
    echo "qemu-system-riscv64 not on PATH — run via 'nix develop' or activate direnv" >&2
    exit 3
fi

mkdir -p "$(dirname "${LOG_FILE}")"
: > "${LOG_FILE}"

# Capture QEMU's pid so we can SIGTERM it as soon as we see a terminal
# banner — that way the wall-clock cost of a successful run is the cost
# of the test suite, not `$TIMEOUT` seconds of QEMU spin-down.
qemu_cmd=(qemu-system-riscv64
    -machine virt
    -cpu rv64
    -smp "${SMP}"
    -m 3072
    -nographic
    -bios none
    -kernel "${PACKED_IMAGE}"
)

if [[ "${VERBOSE}" -eq 1 ]]; then
    "${qemu_cmd[@]}" 2>&1 | tee "${LOG_FILE}" &
else
    "${qemu_cmd[@]}" > "${LOG_FILE}" 2>&1 &
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
while [[ $(date +%s) -lt ${deadline} ]]; do
    if ! kill -0 "${qemu_pid}" 2>/dev/null; then
        # QEMU exited on its own (rootserver called shutdown or the image
        # finished). Drain any final output then classify.
        wait "${qemu_pid}" 2>/dev/null || true
        break
    fi
    # Look for the pass/fail banners written so far.
    if grep -qE 'Test suite passed\.' "${LOG_FILE}" 2>/dev/null; then
        status=0
        break
    fi
    if grep -qE 'Test suite failed|seL4 root server abort|\*\*\* KERNEL PANIC' "${LOG_FILE}" 2>/dev/null; then
        status=1
        break
    fi
    sleep 0.5
done

cleanup
trap - EXIT INT TERM

if [[ ${status} -eq 2 ]] && grep -qE 'Test suite passed\.' "${LOG_FILE}" 2>/dev/null; then
    status=0
fi

passed_line="$(grep -E 'Test suite passed\..*tests passed' "${LOG_FILE}" | tail -1 || true)"
test_count="$(grep -cE '^Starting test [0-9]+:' "${LOG_FILE}" || true)"

case "${status}" in
    0)
        echo "PASS: ${passed_line:-Test suite passed.}"
        echo "      log: ${LOG_FILE}"
        ;;
    1)
        echo "FAIL: test suite reported failure or kernel aborted"
        echo "      saw ${test_count} 'Starting test ...' lines"
        echo "      tail of log:"
        tail -20 "${LOG_FILE}" | sed 's/^/        /'
        ;;
    2)
        echo "TIMEOUT after ${TIMEOUT}s without seeing pass/fail banner"
        echo "      saw ${test_count} 'Starting test ...' lines"
        echo "      tail of log:"
        tail -20 "${LOG_FILE}" | sed 's/^/        /'
        ;;
esac

exit "${status}"
