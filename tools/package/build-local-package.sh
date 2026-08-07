#!/usr/bin/env bash
set -euo pipefail

root=$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)
package_dir="$root/packaging"
run_checks=true
check_system_dependencies=true

usage() {
  cat <<'EOF'
Usage: tools/package/build-local-package.sh [--no-check] [--skip-system-dependency-check]

Build and validate local Splinterm Arch packages.

  --no-check                      Skip the PKGBUILD check() function
  --skip-system-dependency-check  CI only: let makepkg skip host package checks
  -h, --help   Show this help
EOF
}

while (($#)); do
  case $1 in
    --no-check) run_checks=false ;;
    --skip-system-dependency-check) check_system_dependencies=false ;;
    -h|--help) usage; exit 0 ;;
    *) printf 'unknown argument: %s\n' "$1" >&2; usage >&2; exit 2 ;;
  esac
  shift
done

pkgver=$(bash -c "source '$package_dir/PKGBUILD'; printf '%s' \"\$pkgver\"")
archive="$package_dir/splinterm-$pkgver.tar.gz"

if [[ -n $(git -C "$root" status --porcelain --untracked-files=no -- . ':(exclude)AGENTS.md') ]]; then
  printf 'package inputs contain uncommitted tracked changes\n' >&2
  exit 1
fi

if [[ $check_system_dependencies == true ]]; then
  required=(appstream cargo desktop-file-utils fontconfig freetype2 gcc-libs glibc
    hicolor-icon-theme libxkbcommon noto-fonts-cjk noto-fonts-emoji pixman pkgconf
    python rust ttf-jetbrains-mono-nerd-basic wayland xdg-terminal-exec)
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
python "$root/tools/package/validate-package.py" "$package" --mcp-package "$mcp_package"
printf 'Private packages ready:\n  %s\n  %s\n' "$package" "$mcp_package"
