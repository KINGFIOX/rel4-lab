#!/usr/bin/env bash

xv6_acquire_build_lock() {
    if [[ "${XV6_BUILD_LOCK_HELD:-0}" == "1" ]]; then
        XV6_BUILD_LOCK_ACQUIRED=0
        return 0
    fi

    local lock_dir="${XV6_BUILD_LOCK_DIR:-${ROOT_DIR}/target/xv6compat/.build.lock}"
    mkdir -p "$(dirname "${lock_dir}")"
    while ! mkdir "${lock_dir}" 2>/dev/null; do
        if [[ -f "${lock_dir}/pid" ]]; then
            local holder
            holder="$(cat "${lock_dir}/pid" 2>/dev/null || true)"
            if [[ "${holder}" =~ ^[0-9]+$ ]] && ! kill -0 "${holder}" 2>/dev/null; then
                rm -rf "${lock_dir}"
                continue
            fi
        fi
        sleep 0.2
    done

    printf '%s\n' "$$" >"${lock_dir}/pid"
    XV6_BUILD_LOCK_DIR_ACTIVE="${lock_dir}"
    XV6_BUILD_LOCK_ACQUIRED=1
    export XV6_BUILD_LOCK_HELD=1
}

xv6_release_build_lock() {
    if [[ "${XV6_BUILD_LOCK_ACQUIRED:-0}" != "1" ]]; then
        return 0
    fi
    rm -rf "${XV6_BUILD_LOCK_DIR_ACTIVE}"
    XV6_BUILD_LOCK_ACQUIRED=0
    unset XV6_BUILD_LOCK_HELD
}
