#!/usr/bin/env bash
set -euo pipefail

readonly expected_commit="3c5b584b0eafa772eb4376fb6eaf6643399e190e"
readonly source_dir="${FOOT_SOURCE:-$HOME/Playground/foot}"
readonly worktree_dir="${FOOT_ORACLE_WORKTREE:-/tmp/splinterm-foot-oracle-worktree}"
readonly build_dir="${FOOT_ORACLE_BUILD:-/tmp/splinterm-foot-oracle-build}"
readonly project_root=$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)
readonly patch_file="$project_root/tools/foot-oracle/patches/0001-semantic-state-dump.patch"

if [[ ! -d "$source_dir/.git" ]]; then
  printf 'Foot source is not a Git checkout: %s\n' "$source_dir" >&2
  exit 1
fi

actual_commit=$(git -C "$source_dir" rev-parse HEAD)
if [[ "$actual_commit" != "$expected_commit" ]]; then
  printf 'Foot revision mismatch.\nExpected: %s\nActual:   %s\n' \
    "$expected_commit" "$actual_commit" >&2
  exit 1
fi

if [[ ! -f "$patch_file" ]]; then
  printf 'Oracle patch is missing: %s\n' "$patch_file" >&2
  exit 1
fi

if [[ -e "$worktree_dir" ]]; then
  git -C "$source_dir" worktree remove --force "$worktree_dir" 2>/dev/null || \
    rm -rf -- "$worktree_dir"
fi
git -C "$source_dir" worktree prune
git -C "$source_dir" worktree add --detach "$worktree_dir" "$expected_commit"
git -C "$worktree_dir" apply --check "$patch_file"
git -C "$worktree_dir" apply "$patch_file"

rm -rf -- "$build_dir"
meson setup "$build_dir" "$worktree_dir" \
  -Ddocs=disabled \
  -Dthemes=false \
  -Dterminfo=disabled \
  -Dutmp-backend=none \
  -Dgrapheme-clustering=disabled \
  -Dtests=true
meson compile -C "$build_dir"
meson test -C "$build_dir" --print-errorlogs

printf 'Patched Foot semantic oracle built at %s/foot\n' "$build_dir"
