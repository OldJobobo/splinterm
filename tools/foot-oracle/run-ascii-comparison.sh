#!/usr/bin/env bash
set -euo pipefail

root=$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)
output_dir=${1:-/tmp/splinterm-ascii-comparison}
mkdir -p "$output_dir"

"$root/tools/foot-oracle/run-fcft-mask-probe.sh" \
  > "$output_dir/fcft-reference.jsonl"
cargo run --quiet --manifest-path "$root/Cargo.toml" -p splinterm \
  --example ascii-glyph-evidence \
  > "$output_dir/swash-actual.jsonl"
cargo run --quiet --manifest-path "$root/Cargo.toml" -p splinterm \
  --example ascii-freetype-evidence \
  > "$output_dir/freetype-actual.jsonl"
cargo run --quiet --manifest-path "$root/Cargo.toml" -p splinterm \
  --example ascii-production-evidence \
  > "$output_dir/production-actual.jsonl"

set +e
python "$root/tools/foot-oracle/compare-glyph-masks.py" \
  --reference "$output_dir/fcft-reference.jsonl" \
  --actual "$output_dir/swash-actual.jsonl" \
  --label-prefix ASCII-U+ \
  --output-dir "$output_dir/swash-diff"
swash_status=$?
python "$root/tools/foot-oracle/compare-glyph-masks.py" \
  --reference "$output_dir/fcft-reference.jsonl" \
  --actual "$output_dir/freetype-actual.jsonl" \
  --label-prefix ASCII-U+ \
  --output-dir "$output_dir/freetype-diff"
freetype_status=$?
python "$root/tools/foot-oracle/compare-glyph-masks.py" \
  --reference "$output_dir/fcft-reference.jsonl" \
  --actual "$output_dir/production-actual.jsonl" \
  --label-prefix ASCII-U+ \
  --output-dir "$output_dir/production-diff"
production_status=$?
set -e

printf 'Swash comparison status: %d (known provisional path)\n' "$swash_status"
printf 'FreeType comparison status: %d (bridge isolation gate)\n' "$freetype_status"
printf 'Production comparison status: %d (snapshot cache gate)\n' "$production_status"
printf 'Phase 8.1 ASCII evidence: %s\n' "$output_dir"
if (( freetype_status != 0 || production_status != 0 )); then
  exit 1
fi
