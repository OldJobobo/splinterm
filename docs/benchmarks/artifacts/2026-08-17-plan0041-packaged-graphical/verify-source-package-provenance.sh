#!/usr/bin/env bash
set -euo pipefail

repo=${1:?usage: verify-source-package-provenance.sh REPO PACKAGE SOURCE_ARCHIVE}
package=${2:?usage: verify-source-package-provenance.sh REPO PACKAGE SOURCE_ARCHIVE}
source_archive=${3:?usage: verify-source-package-provenance.sh REPO PACKAGE SOURCE_ARCHIVE}
artifact_dir=$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")" && pwd)
build_commit=450f6cbbdd17d01b8ee4d3b89b70e084e474aa4d
merged_commit=d3cbbc6b013ff4cab28a8ec6ef6aaa267603ff35
expected_package=22f21317c17bbcfc510d99f6fdb9f3a593e7ab08736bcb8180527c8e2cac3c0b

[[ $(git -C "$repo" rev-parse "$build_commit^{tree}") == \
   $(git -C "$repo" rev-parse "$merged_commit^{tree}") ]]

set +o pipefail
archive_commit=$(gzip -dc -- "$source_archive" | git get-tar-commit-id)
set -o pipefail
[[ $archive_commit == "$build_commit" ]]
[[ $(sha256sum "$package" | cut -d' ' -f1) == "$expected_package" ]]

pkgbuild=$(mktemp)
trap 'rm -f -- "$pkgbuild"' EXIT
git -C "$repo" show "$build_commit:packaging/PKGBUILD" > "$pkgbuild"
[[ $(sha256sum "$pkgbuild" | cut -d' ' -f1) == \
   $(sed -n 's/^pkgbuild_sha256sum = //p' "$artifact_dir/package.BUILDINFO") ]]

for member in usr/bin/splinterm usr/bin/splinterd usr/bin/splinterm-pty-child; do
  expected=$(awk -v member="$member" '$1 == member { print $2 }' \
    "$artifact_dir/package-member-sha256.txt")
  actual=$(bsdtar -xOf "$package" "$member" | sha256sum | cut -d' ' -f1)
  [[ -n $expected && $actual == "$expected" ]]
done

printf '%s\n' 'Plan 0041 source/package provenance verified'
