#!/usr/bin/env bash
set -euo pipefail

readonly expected_commit="3c5b584b0eafa772eb4376fb6eaf6643399e190e"
readonly source_dir="${FOOT_SOURCE:-$HOME/Playground/foot}"
readonly build_dir="${FOOT_BUILD:-/tmp/splinterm-foot-build}"

if [[ ! -d "$source_dir/.git" ]]; then
  printf 'Foot source is not a Git checkout: %s\n' "$source_dir" >&2
  exit 1
fi

actual_commit=$(git -C "$source_dir" rev-parse HEAD)
if [[ "$actual_commit" != "$expected_commit" && "${ALLOW_FOOT_REVISION_MISMATCH:-0}" != "1" ]]; then
  printf 'Foot revision mismatch.\nExpected: %s\nActual:   %s\n' \
    "$expected_commit" "$actual_commit" >&2
  printf 'Set ALLOW_FOOT_REVISION_MISMATCH=1 only for deliberate experiments.\n' >&2
  exit 1
fi

rm -rf -- "$build_dir"
meson setup "$build_dir" "$source_dir" \
  -Ddocs=disabled \
  -Dthemes=false \
  -Dterminfo=disabled \
  -Dutmp-backend=none \
  -Dgrapheme-clustering=disabled \
  -Dtests=true
meson compile -C "$build_dir"
meson test -C "$build_dir" --print-errorlogs

printf 'Foot reference built and tested at %s\n' "$build_dir"
