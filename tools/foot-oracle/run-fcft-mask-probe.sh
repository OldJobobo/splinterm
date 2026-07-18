#!/usr/bin/env bash
set -euo pipefail

root=$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)
foot_source=${FOOT_SOURCE:-$HOME/Playground/foot}
build_dir=${FOOT_REFERENCE_BUILD_DIR:-/tmp/splinterm-foot-build}
expected=3c5b584b0eafa772eb4376fb6eaf6643399e190e

actual=$(git -C "$foot_source" rev-parse HEAD)
if [[ $actual != "$expected" ]]; then
  printf 'Foot source is at %s; expected %s\n' "$actual" "$expected" >&2
  exit 2
fi
if [[ ! -f $build_dir/subprojects/fcft/libfcft.a ]]; then
  "$root/tools/foot-oracle/build-reference.sh"
fi

binary=$build_dir/fcft-mask-probe
cc \
  -std=c11 -O2 -D_GNU_SOURCE=200809L \
  -I"$foot_source/subprojects/fcft" \
  -I"$build_dir/subprojects/fcft" \
  -I/usr/include/pixman-1 \
  -I/usr/include/freetype2 \
  -I/usr/include/harfbuzz \
  "$root/tools/foot-oracle/fcft-mask-probe.c" \
  "$build_dir/subprojects/fcft/libfcft.a" \
  -Wl,--start-group \
  -lpixman-1 -lfontconfig -lfreetype -lharfbuzz -lutf8proc -lm -pthread \
  -Wl,--end-group \
  -o "$binary"

exec "$binary"
