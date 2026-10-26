#!/usr/bin/env bash
#
# Copyright (c) 2026 Contributors to the Eclipse Foundation
#
# See the NOTICE file(s) distributed with this work for additional
# information regarding copyright ownership.
#
# This program and the accompanying materials are made available under the
# terms of the Apache License Version 2.0 which is available at
# https://www.apache.org/licenses/LICENSE-2.0
#
# SPDX-License-Identifier: Apache-2.0

set -euo pipefail

repo_dir="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
bazelisk_version="$(tr -d '[:space:]' < "${repo_dir}/tools/bazelisk.version")"
bazelisk_sha="$(tr -d '[:space:]' < "${repo_dir}/tools/bazelisk-linux-amd64.sha256")"
tool_dir="${BAZELISK_CACHE_DIR:-${repo_dir}/.cache/tools}"
bazelisk="${tool_dir}/bazelisk-${bazelisk_version}-linux-amd64"

ensure_bazelisk() {
    mkdir -p "${tool_dir}"
    if [[ ! -x "${bazelisk}" ]]; then
        curl -fsSL \
            -o "${bazelisk}.tmp" \
            "https://github.com/bazelbuild/bazelisk/releases/download/${bazelisk_version}/bazelisk-linux-amd64"
        actual_sha="$(sha256sum "${bazelisk}.tmp" | cut -d ' ' -f 1)"
        if [[ "${actual_sha}" != "${bazelisk_sha}" ]]; then
            rm -f "${bazelisk}.tmp"
            printf 'Bazelisk checksum mismatch: expected %s, got %s\n' "${bazelisk_sha}" "${actual_sha}" >&2
            exit 1
        fi
        chmod +x "${bazelisk}.tmp"
        mv "${bazelisk}.tmp" "${bazelisk}"
    else
        actual_sha="$(sha256sum "${bazelisk}" | cut -d ' ' -f 1)"
        if [[ "${actual_sha}" != "${bazelisk_sha}" ]]; then
            printf 'Cached Bazelisk checksum mismatch: expected %s, got %s\n' "${bazelisk_sha}" "${actual_sha}" >&2
            exit 1
        fi
    fi
}

usage() {
    cat <<'USAGE'
Usage: scripts/ensure-lola-bazelisk.sh [--print|--version]

Downloads the pinned Bazelisk binary used by LoLa bundled validation into
up-streamer-rust/.cache/tools unless BAZELISK_CACHE_DIR overrides the cache.
USAGE
}

mode="${1:---print}"
case "${mode}" in
    --print)
        ensure_bazelisk
        printf '%s\n' "${bazelisk}"
        ;;
    --version)
        ensure_bazelisk
        "${bazelisk}" --version
        ;;
    -h|--help)
        usage
        ;;
    *)
        usage >&2
        exit 2
        ;;
esac
