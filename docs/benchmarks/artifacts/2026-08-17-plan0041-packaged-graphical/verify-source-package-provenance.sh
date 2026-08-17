#!/usr/bin/env bash
set -euo pipefail

repo=${1:?usage: verify-source-package-provenance.sh REPO PACKAGE SOURCE_ARCHIVE}
package=${2:?usage: verify-source-package-provenance.sh REPO PACKAGE SOURCE_ARCHIVE}
source_archive=${3:?usage: verify-source-package-provenance.sh REPO PACKAGE SOURCE_ARCHIVE}
artifact_dir=$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")" && pwd)
merged_commit=d3cbbc6b013ff4cab28a8ec6ef6aaa267603ff35
historical_archive_commit=450f6cbbdd17d01b8ee4d3b89b70e084e474aa4d
expected_tree=48d6418ce85bd19d3519396f2542ad04e4421d86
expected_source_archive=93dec79b89096d7c64fc25151ee4a428d2ff4e37faad0e7a1d1975a1dc0d9f02
expected_package=22f21317c17bbcfc510d99f6fdb9f3a593e7ab08736bcb8180527c8e2cac3c0b

[[ $(git -C "$repo" rev-parse "$merged_commit^{tree}") == "$expected_tree" ]]
[[ $(sha256sum "$source_archive" | cut -d' ' -f1) == "$expected_source_archive" ]]
[[ $(sha256sum "$package" | cut -d' ' -f1) == "$expected_package" ]]

set +o pipefail
archive_commit=$(gzip -dc -- "$source_archive" | git get-tar-commit-id)
set -o pipefail
[[ $archive_commit == "$historical_archive_commit" ]]

scratch=$(mktemp -d)
trap 'rm -rf -- "$scratch"' EXIT
tar -xzf "$source_archive" -C "$scratch"
git -C "$repo" archive --format=tar --prefix=merged/ "$merged_commit" |
  tar -xf - -C "$scratch"
diff -qr "$scratch/splinterm-0.1.0alpha3.3" "$scratch/merged"

git -C "$repo" show "$merged_commit:packaging/PKGBUILD" > "$scratch/PKGBUILD"
[[ $(sha256sum "$scratch/PKGBUILD" | cut -d' ' -f1) == \
   $(sed -n 's/^pkgbuild_sha256sum = //p' "$artifact_dir/package.BUILDINFO") ]]
[[ $(git -C "$repo" rev-parse "$merged_commit:tools/package/build-local-package.sh") == \
   336c1882942eb99fe31b28be81964f8f889cc221 ]]

for member in usr/bin/splinterm usr/bin/splinterd usr/bin/splinterm-pty-child; do
  expected=$(awk -v member="$member" '$1 == member { print $2 }' \
    "$artifact_dir/package-member-sha256.txt")
  actual=$(bsdtar -xOf "$package" "$member" | sha256sum | cut -d' ' -f1)
  [[ -n $expected && $actual == "$expected" ]]
done

printf '%s\n' 'Plan 0041 source/package provenance verified'
