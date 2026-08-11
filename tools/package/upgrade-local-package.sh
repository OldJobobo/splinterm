#!/usr/bin/env bash
set -euo pipefail

root=$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)
package_dir="$root/packaging"
build=false
confirm=true

usage() {
  cat <<'EOF'
Usage: tools/package/upgrade-local-package.sh [--build] [-y|--yes]

Validate and install the newest local Splinterm Arch package.

  --build      First build a package from the clean, committed checkout
  -y, --yes    Skip the confirmation prompt
  -h, --help   Show this help
EOF
}

while (($#)); do
  case $1 in
    --build) build=true ;;
    -y|--yes) confirm=false ;;
    -h|--help) usage; exit 0 ;;
    *) printf 'unknown argument: %s\n' "$1" >&2; usage >&2; exit 2 ;;
  esac
  shift
done

refuse_daemon_owned_invocation() {
  local proc_root=${SPLINTERM_INSTALL_PROC_ROOT:-/proc}
  local pid=${SPLINTERM_INSTALL_PARENT_PID:-$PPID}
  local depth=0 executable parent
  while [[ $pid =~ ^[0-9]+$ ]] && ((pid > 1 && depth < 64)); do
    executable=$(readlink -f "$proc_root/$pid/exe" 2>/dev/null || true)
    case ${executable##*/} in
      splinterd|splinterm-pty-child)
        printf '%s\n' \
          'Refusing to install from inside a Splinterm-owned shell.' \
          'Stopping splinterd would terminate this installer before it can finish.' \
          'Run the installer from Foot or another terminal not owned by splinterd.' >&2
        exit 1
        ;;
    esac
    parent=$(awk '/^PPid:/ { print $2; exit }' "$proc_root/$pid/status" 2>/dev/null || true)
    pid=$parent
    depth=$((depth + 1))
  done
}

refuse_daemon_owned_invocation

if [[ $build == true ]]; then
  "$root/tools/package/build-local-package.sh"
fi

package=$(
  find "$package_dir" -maxdepth 1 -type f \
    -name 'splinterm-[0-9]*.pkg.tar.*' \
    -printf '%T@ %p\n' |
    sort -nr |
    head -n 1 |
    cut -d' ' -f2-
)
[[ -n $package ]] || {
  printf 'no Splinterm package found; run tools/package/build-local-package.sh first\n' >&2
  exit 1
}

mcp_package=$(
  find "$package_dir" -maxdepth 1 -type f \
    -name 'splinterm-mcp-[0-9]*.pkg.tar.*' \
    -printf '%T@ %p\n' |
    sort -nr |
    head -n 1 |
    cut -d' ' -f2-
)
[[ -n $mcp_package ]] || {
  printf 'no matching optional MCP package found; rebuild the split package set\n' >&2
  exit 1
}
python "$root/tools/package/validate-package.py" "$package" --mcp-package "$mcp_package"

installed=$(pacman -Q splinterm 2>/dev/null | awk '{print $2}' || true)
mcp_installed=$(pacman -Q splinterm-mcp 2>/dev/null | awk '{print $2}' || true)
candidate=$(pacman -Qp "$package" | awk '{print $2}')
packages=("$package")
mcp_action='not installed (left opt-in)'
if [[ -n $mcp_installed ]]; then
  packages+=("$mcp_package")
  mcp_action="upgrade $mcp_package"
fi
was_active=false
systemctl --user is-active --quiet splinterd.service && was_active=true

printf 'Installed: %s\nCandidate: %s\nPackage:   %s\nOptional MCP: %s\n' \
  "${installed:-not installed}" "$candidate" "$package" "$mcp_action"
if [[ $was_active == true ]]; then
  printf 'splinterd is running; upgrading will end its daemon-owned shells.\n'
fi

if [[ $confirm == true ]]; then
  if command -v gum >/dev/null 2>&1; then
    gum confirm 'Install this local Splinterm package?' || exit 0
  elif [[ -t 0 ]]; then
    read -r -p 'Install this local Splinterm package? [y/N] ' answer
    [[ $answer == [yY] || $answer == [yY][eE][sS] ]] || exit 0
  else
    printf 'confirmation requires a terminal; rerun with --yes\n' >&2
    exit 1
  fi
fi

stopped=false
restore_daemon() {
  if [[ $stopped == true ]]; then
    systemctl --user daemon-reload || true
    systemctl --user start splinterd.service || true
  fi
}
trap restore_daemon EXIT

if [[ $was_active == true ]]; then
  systemctl --user stop splinterd.service
  stopped=true
fi

# This is an interactive terminal workflow, so sudo follows Omarchy's convention.
sudo pacman -U --noconfirm "${packages[@]}"
systemctl --user daemon-reload

if [[ $was_active == true ]]; then
  systemctl --user start splinterd.service
  stopped=false
fi

trap - EXIT
printf 'Splinterm %s installed.\n' "$candidate"
