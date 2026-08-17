#!/usr/bin/env bash
set -euo pipefail

source_archive=${1:?usage: rebuild-source-package-members.sh SOURCE_ARCHIVE}
artifact_dir=$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")" && pwd)
expected_source_archive=93dec79b89096d7c64fc25151ee4a428d2ff4e37faad0e7a1d1975a1dc0d9f02
[[ $(sha256sum "$source_archive" | cut -d' ' -f1) == "$expected_source_archive" ]]

scratch=$(mktemp -d)
trap 'rm -rf -- "$scratch"' EXIT
cp -- "$artifact_dir/package.PKGBUILD" "$scratch/PKGBUILD"
cp -- "$artifact_dir/package.install" "$scratch/splinterm.install"
cp -- "$source_archive" "$scratch/splinterm-0.1.0alpha3.3.tar.gz"
(
  cd "$scratch"
  makepkg --cleanbuild --clean --noconfirm --noprogressbar --nocheck
)
rebuilt=$(find "$scratch" -maxdepth 1 \
  -name 'splinterm-0.1.0alpha3.3-1-x86_64.pkg.tar.*' -print -quit)
[[ -n $rebuilt ]]

for member in usr/bin/splinterm usr/bin/splinterd usr/bin/splinterm-pty-child; do
  expected=$(awk -v member="$member" '$1 == member { print $2 }' \
    "$artifact_dir/package-member-sha256.txt")
  actual=$(bsdtar -xOf "$rebuilt" "$member" | sha256sum | cut -d' ' -f1)
  printf '%s %s\n' "$member" "$actual"
  [[ -n $expected && $actual == "$expected" ]]
done

printf '%s\n' 'Plan 0041 source rebuild matches tested package members'
