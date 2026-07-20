#!/usr/bin/env bash
set -euo pipefail

root=$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)
package_dir="$root/packaging"
pkgver=$(bash -c "source '$package_dir/PKGBUILD'; printf '%s' \"\$pkgver\"")
archive="$package_dir/splinterm-$pkgver.tar.gz"

if [[ -n $(git -C "$root" status --porcelain --untracked-files=no -- . ':(exclude)AGENTS.md') ]]; then
  printf 'package inputs contain uncommitted tracked changes\n' >&2
  exit 1
fi

required=(cargo fontconfig freetype2 gcc-libs glibc hicolor-icon-theme libxkbcommon
  noto-fonts-cjk noto-fonts-emoji pixman pkgconf python rust
  ttf-jetbrains-mono-nerd-basic wayland
  xdg-terminal-exec)
missing=$(pacman -T "${required[@]}" || true)
if [[ -n "$missing" ]]; then
  printf 'missing package dependencies:\n%s\n' "$missing" >&2
  exit 1
fi

rm -rf "$package_dir/src" "$package_dir/pkg" "$archive"
rm -f "$package_dir"/splinterm-*.pkg.tar.*
git -C "$root" archive --format=tar.gz --prefix="splinterm-$pkgver/" -o "$archive" HEAD
(
  cd "$package_dir"
  makepkg --cleanbuild --clean --noconfirm --noprogressbar
)
package=$(find "$package_dir" -maxdepth 1 -name 'splinterm-*.pkg.tar.*' -print -quit)
[[ -n "$package" ]] || { printf 'makepkg produced no package\n' >&2; exit 1; }
python "$root/tools/package/validate-package.py" "$package"
printf 'Private package ready: %s\n' "$package"
