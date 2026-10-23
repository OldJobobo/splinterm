#!/usr/bin/env bash
set -euo pipefail

root=$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)
package_dir="$root/packaging"
build=false
confirm=true
validate_args=()

usage() {
  cat <<'EOF'
Usage: tools/package/upgrade-local-package.sh [--build] [-y|--yes]
                                              [--allow-vm-acceptance-live-handoff]

Validate and install the newest local Splinterm Arch package.

  --build      First build a package from the clean, committed checkout
  -y, --yes    Skip the confirmation prompt
  --allow-vm-acceptance-live-handoff
               TEST-ONLY: accept a local VM acceptance package whose daemon
               is a VM acceptance build (VM testbed only)
  -h, --help   Show this help
EOF
}

while (($#)); do
  case $1 in
    --build) build=true ;;
    -y|--yes) confirm=false ;;
    --allow-vm-acceptance-live-handoff)
      # TEST-ONLY packages belong in the VM testbed only, whose package-install
      # action sets SPLINTERM_TESTBED_PACKAGE_ROOT.
      [[ -n ${SPLINTERM_TESTBED_PACKAGE_ROOT:-} ]] || {
        printf 'refusing --allow-vm-acceptance-live-handoff outside the VM testbed (SPLINTERM_TESTBED_PACKAGE_ROOT is unset)\n' >&2
        exit 1
      }
      validate_args+=(--allow-vm-acceptance-live-handoff) ;;
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

# Says what installing does to the running daemon, from the daemon's own report
# when it can give one (Plan 0037 Milestone 4). Read-only: `splinterm upgrade`
# without --now only reports. A 0.1.x daemon, or a client that predates the
# report, cannot count its shells; then the running Splint workload scopes give
# a best-effort count, and every running shell is still assumed to end.
describe_running_daemon() {
  local client=${SPLINTERM_INSTALL_CLIENT:-/usr/bin/splinterm} report count scopes
  report=$(timeout 5 "$client" upgrade 2>/dev/null) || report=
  count=$(sed -n 's/^Sessions: .* \([0-9][0-9]*\) live terminal(s)$/\1/p' <<<"$report" | head -n 1)
  case $count in
    '')
      scopes=$(systemctl --user list-units --type=scope --state=active --plain \
        --no-legend --no-pager 'splinterm-splint*.scope' 2>/dev/null |
        awk '$1 ~ /^splinterm-splint.*\.scope$/ { n++ } END { print n + 0 }') || scopes=0
      printf 'splinterd is running and cannot report its live terminals (a 0.1.x daemon cannot).\n'
      if ((scopes > 0)); then
        printf 'About %s shells appear to be running (a best-effort count of running workload units).\n' "$scopes"
      fi
      printf 'Installing stops it, which ends every running shell; a package install never hands them to the new build.\n'
      ;;
    0) printf 'splinterd is running with no live terminals; installing restarts it and ends nothing.\n' ;;
    1) printf 'splinterd is running with 1 live terminal; installing stops it and ends that shell.\n' ;;
    *) printf 'splinterd is running with %s live terminals; installing stops it and ends all of them.\n' "$count" ;;
  esac
  printf '%s\n' \
    'Open Splinterm Windows must be reopened after the install.' \
    'Cancel to leave the daemon and its shells untouched; nothing has changed yet.'
}

# Names a downgrade before confirmation. An older daemon cannot read saved
# topology written in a newer format; it renames those files to
# topology.invalid-* in its state directory and starts without them. The 0.1
# launcher restarts an incompatible daemon without asking, so a downgrade to
# 0.1.x is said plainly.
describe_version_change() {
  local installed=$1 candidate=$2
  [[ -n $installed && -n $candidate ]] && command -v vercmp >/dev/null 2>&1 || return 0
  if (($(vercmp "$candidate" "$installed") < 0)); then
    printf '%s\n' \
      "This is a downgrade from $installed to $candidate." \
      'Saved topology written in a newer format is set aside (renamed to topology.invalid-*), not read.'
    if [[ $candidate == 0.[01].* ]]; then
      printf '%s\n' \
        'The 0.1 launcher restarts an incompatible daemon without asking, so this installer stops the' \
        'daemon here first; never downgrade to 0.1.x with pacman alone while shells run.'
    fi
  fi
}

# Authenticates sudo before anything stops, so a refused or failed sudo ends
# no shell.
authenticate_sudo() {
  sudo -v || {
    printf 'sudo authentication failed; nothing was installed, stopped, or restarted.\n' >&2
    exit 1
  }
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
python "$root/tools/package/validate-package.py" "$package" --mcp-package "$mcp_package" \
  "${validate_args[@]}"

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
describe_version_change "$installed" "$candidate"
if [[ $was_active == true ]]; then
  describe_running_daemon
else
  printf 'splinterd is not running; installing ends nothing.\n'
fi

cancelled() {
  printf 'Cancelled; nothing was installed, stopped, or restarted.\n'
  exit 0
}

if [[ $confirm == true ]]; then
  if command -v gum >/dev/null 2>&1; then
    gum confirm 'Install this local Splinterm package?' || cancelled
  elif [[ -t 0 ]]; then
    read -r -p 'Install this local Splinterm package? [y/N] ' answer
    [[ $answer == [yY] || $answer == [yY][eE][sS] ]] || cancelled
  else
    printf 'confirmation requires a terminal; rerun with --yes\n' >&2
    exit 1
  fi
fi

authenticate_sudo

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
