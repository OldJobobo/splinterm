#!/usr/bin/env bash
set -euo pipefail

root=$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)
assume_yes=false
run_checks=false

usage() {
  cat <<'EOF'
Usage: ./install.sh [--check] [-y|--yes]

Build and install Splinterm on an Arch/Omarchy system.

  --check      Run the full package test suite before installation
  -y, --yes    Skip Pacman and final installation confirmations
  -h, --help   Show this help

This installs Splinterm without changing the default terminal or Omarchy
configuration. It never opts you into the optional MCP package; an existing
MCP installation is upgraded to keep its version matched.
EOF
}

while (($#)); do
  case $1 in
    --check) run_checks=true ;;
    -y|--yes) assume_yes=true ;;
    -h|--help) usage; exit 0 ;;
    *) printf 'Unknown argument: %s\n\n' "$1" >&2; usage >&2; exit 2 ;;
  esac
  shift
done

if ((EUID == 0)); then
  printf 'Run this script as your normal user; it invokes sudo only for Pacman.\n' >&2
  exit 1
fi
if [[ $(uname -m) != x86_64 ]] || ! command -v pacman >/dev/null 2>&1; then
  printf 'This installer supports x86_64 Arch/Omarchy systems.\n' >&2
  exit 1
fi
for command in git sudo; do
  command -v "$command" >/dev/null 2>&1 || {
    printf 'Required command not found: %s\n' "$command" >&2
    exit 1
  }
done
if [[ -n $(git -C "$root" status --porcelain --untracked-files=no -- . ':(exclude)AGENTS.md') ]]; then
  printf 'The checkout has uncommitted tracked changes. Commit or discard them first.\n' >&2
  exit 1
fi

printf '%s\n' \
  'Splinterm local install' \
  '  Build:   current committed checkout' \
  '  Install: Splinterm Arch package' \
  '  Leave:   default terminal and Omarchy configuration unchanged'

required=(appstream base-devel cargo desktop-file-utils fontconfig freetype2
  gcc-libs glibc hicolor-icon-theme libxkbcommon noto-fonts-cjk noto-fonts-emoji
  pixman pkgconf python rust ttf-jetbrains-mono-nerd-basic wayland
  xdg-terminal-exec)
missing=$(pacman -T "${required[@]}" 2>/dev/null || true)
if [[ -n $missing ]] || ! command -v makepkg >/dev/null 2>&1; then
  printf '\n[1/3] Installing build and runtime dependencies\n'
  pacman_args=(-S --needed)
  [[ $assume_yes == true ]] && pacman_args+=(--noconfirm)
  sudo pacman "${pacman_args[@]}" "${required[@]}"
else
  printf '\n[1/3] Dependencies ready\n'
fi

printf '\n[2/3] Building and validating packages\n'
build_args=()
[[ $run_checks == false ]] && build_args+=(--no-check)
"$root/tools/package/build-local-package.sh" "${build_args[@]}"

printf '\n[3/3] Installing Splinterm\n'
install_args=()
[[ $assume_yes == true ]] && install_args+=(--yes)
"$root/tools/package/upgrade-local-package.sh" "${install_args[@]}"
