#!/usr/bin/env bash
set -euo pipefail

root=$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)
package_dir="$root/packaging"
run_checks=true
check_system_dependencies=true
vm_acceptance=false

usage() {
  cat <<'EOF'
Usage: tools/package/build-local-package.sh [--no-check] [--skip-system-dependency-check]
                                            [--vm-acceptance-live-handoff]

Build and validate local Splinterm Arch packages.

  --no-check                      Skip the PKGBUILD check() function
  --skip-system-dependency-check  CI only: let makepkg skip host package checks
  --vm-acceptance-live-handoff    TEST-ONLY: build the daemon with the
                                  vm-acceptance-live-handoff feature, which
                                  forces live handoff on, for VM acceptance
                                  only (local-committed-head provenance; never
                                  publish or ship the result)
  -h, --help   Show this help
EOF
}

# Derives the TEST-ONLY VM acceptance build script from the checked-in
# PKGBUILD, which never enables the feature: the daemon is built with
# `--features splinterd/vm-acceptance-live-handoff`, the provenance record is
# generated with `--allow-vm-acceptance-live-handoff`, and the package
# description says it is test-only. Each replaced line must occur exactly once.
derive_vm_acceptance_buildscript() {
  local source=$1 target=$2
  local build_line="  cargo build --frozen --release \\"
  local provenance_line="  python tools/package/generate-package-provenance.py \\"
  local description_line="  pkgdesc='Persistent Wayland terminal for humans and bounded automation'"
  local line
  for line in "$build_line" "$provenance_line" "$description_line"; do
    [[ $(grep -cxF -- "$line" "$source") == 1 ]] || {
      printf 'cannot derive the VM acceptance build script: PKGBUILD changed near: %s\n' "$line" >&2
      return 1
    }
  done
  # Through the environment: awk -v would interpret the trailing backslashes.
  VM_BUILD_LINE=$build_line VM_PROVENANCE_LINE=$provenance_line \
    VM_DESCRIPTION_LINE=$description_line awk '
    BEGIN {
      build = ENVIRON["VM_BUILD_LINE"]
      provenance = ENVIRON["VM_PROVENANCE_LINE"]
      description = ENVIRON["VM_DESCRIPTION_LINE"]
    }
    $0 == build {
      print "  cargo build --frozen --release --features splinterd/vm-acceptance-live-handoff \\"
      next
    }
    $0 == provenance { print; print "    --allow-vm-acceptance-live-handoff \\"; next }
    $0 == description {
      print "  pkgdesc='"'"'TEST-ONLY VM acceptance build (live handoff forced on): never ship'"'"'"
      next
    }
    { print }
  ' "$source" >"$target"
}

while (($#)); do
  case $1 in
    --no-check) run_checks=false ;;
    --skip-system-dependency-check) check_system_dependencies=false ;;
    --vm-acceptance-live-handoff) vm_acceptance=true ;;
    -h|--help) usage; exit 0 ;;
    *) printf 'unknown argument: %s\n' "$1" >&2; usage >&2; exit 2 ;;
  esac
  shift
done

pkgver=$(bash -c "source '$package_dir/PKGBUILD'; printf '%s' \"\$pkgver\"")
archive="$package_dir/splinterm-$pkgver.tar.gz"
commit=$(git -C "$root" rev-parse --verify HEAD)
[[ $commit =~ ^[0-9a-f]{40}$ ]] || {
  printf 'package source commit is not one full lowercase Git object ID\n' >&2
  exit 1
}
export SPLINTERM_BUILD_COMMIT=$commit
export SPLINTERM_BUILD_PROVENANCE=${SPLINTERM_BUILD_PROVENANCE:-local-committed-head}
case $SPLINTERM_BUILD_PROVENANCE in
  local-committed-head|release-candidate) ;;
  *) printf 'unsupported package build provenance: %s\n' "$SPLINTERM_BUILD_PROVENANCE" >&2; exit 1 ;;
esac
if [[ $vm_acceptance == true && $SPLINTERM_BUILD_PROVENANCE != local-committed-head ]]; then
  printf 'a VM acceptance live-handoff package is test-only and needs local-committed-head provenance\n' >&2
  exit 1
fi

if [[ -n $(git -C "$root" status --porcelain --untracked-files=no -- . ':(exclude)AGENTS.md') ]]; then
  printf 'package inputs contain uncommitted tracked changes\n' >&2
  exit 1
fi

if [[ $check_system_dependencies == true ]]; then
  required=(appstream cargo desktop-file-utils fontconfig freetype2 gcc-libs glibc
    hicolor-icon-theme libxkbcommon noto-fonts-cjk noto-fonts-emoji pixman pkgconf
    python rust ttf-jetbrains-mono-nerd wayland xdg-terminal-exec)
  missing=$(pacman -T "${required[@]}" || true)
  if [[ -n "$missing" ]]; then
    printf 'missing package dependencies:\n%s\n' "$missing" >&2
    exit 1
  fi
fi

rm -rf "$package_dir/src" "$package_dir/pkg" "$archive"
rm -f "$package_dir"/splinterm-*.pkg.tar.*
git -C "$root" archive --format=tar.gz --prefix="splinterm-$pkgver/" -o "$archive" HEAD
makepkg_args=(--cleanbuild --clean --noconfirm --noprogressbar)
[[ $run_checks == false ]] && makepkg_args+=(--nocheck)
[[ $check_system_dependencies == false ]] && makepkg_args+=(--nodeps)
validate_args=()
if [[ $vm_acceptance == true ]]; then
  buildscript="$package_dir/PKGBUILD.vm-acceptance-live-handoff"
  trap 'rm -f "$buildscript"' EXIT
  derive_vm_acceptance_buildscript "$package_dir/PKGBUILD" "$buildscript"
  makepkg_args+=(-p "$(basename "$buildscript")")
  validate_args+=(--allow-vm-acceptance-live-handoff)
  printf 'TEST-ONLY: building a VM acceptance package whose daemon forces live handoff on.\n' >&2
fi
(
  cd "$package_dir"
  makepkg "${makepkg_args[@]}"
)
package=$(find "$package_dir" -maxdepth 1 -name "splinterm-$pkgver-*.pkg.tar.*" -print -quit)
mcp_package=$(find "$package_dir" -maxdepth 1 -name "splinterm-mcp-$pkgver-*.pkg.tar.*" -print -quit)
[[ -n "$package" && -n "$mcp_package" ]] || {
  printf 'makepkg produced an incomplete split package set\n' >&2
  exit 1
}
python "$root/tools/package/validate-package.py" "$package" --mcp-package "$mcp_package" \
  "${validate_args[@]}"
printf 'Private packages ready:\n  %s\n  %s\n' "$package" "$mcp_package"
if [[ $vm_acceptance == true ]]; then
  printf 'TEST-ONLY VM acceptance packages: install them only in the VM testbed; never publish or ship them.\n'
fi
