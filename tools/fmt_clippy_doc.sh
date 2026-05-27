#!/bin/bash
set -euo pipefail

cargo fmt --check
source build/envsetup.sh highest

if [[ -z "${BAZEL:-}" ]] && ! command -v bazelisk >/dev/null 2>&1 && ! command -v bazel >/dev/null 2>&1; then
    cat >&2 <<'MSG'
All-features Streamer checks enable the LoLa native bridge, which requires Bazelisk or Bazel.
Set BAZEL=/path/to/bazelisk, install bazelisk or bazel on PATH, then rerun this script.
MSG
    exit 1
fi

cargo clippy --workspace --all-targets --all-features -- -D warnings
RUSTDOCFLAGS=-Dwarnings cargo doc -p up-streamer --no-deps --all-features
