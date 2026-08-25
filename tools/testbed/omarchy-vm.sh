#!/usr/bin/env bash
# Remote paths and commands are deliberately quoted and expanded on the client.
# shellcheck disable=SC2029
set -euo pipefail

repo_root=$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")/../.." && pwd)
config_file=${SPLINTERM_TESTBED_CONFIG:-"$repo_root/.splinterm-testbed.env"}

if [[ -r $config_file ]]; then
  # This is an explicitly selected, maintainer-owned shell configuration file.
  # shellcheck source=/dev/null
  source "$config_file"
fi

host=${SPLINTERM_TESTBED_HOST:-127.0.0.1}
port=${SPLINTERM_TESTBED_PORT:-2222}
user=${SPLINTERM_TESTBED_USER:-omarchy}
identity=${SPLINTERM_TESTBED_IDENTITY:-}
known_hosts=${SPLINTERM_TESTBED_KNOWN_HOSTS:-}
remote_root=${SPLINTERM_TESTBED_REMOTE_ROOT:-"/home/$user/Projects/splinterm-testbed"}
qmp_socket=${SPLINTERM_TESTBED_QMP_SOCKET:-}
action=${1:-status}
if (($# > 0)); then
  shift
fi

usage() {
  cat <<'USAGE'
Splinterm Omarchy VM testbed runner

Usage: tools/testbed/omarchy-vm.sh ACTION [ARGS...]

Actions:
  status             Inspect VM, compositor, toolchain, and checkout state
  bootstrap          Install Rust plus the guest-native input control service
  sync               Mirror the current worktree into the isolated guest checkout
  check              Sync and run the normal non-graphical validation boundary
  cargo ARGS...      Sync and run a focused Cargo command in the guest
  ping               Sync and verify the isolated development daemon
  qmp ARGS...         Query or control guest input through the pinned QMP socket
  input ARGS...       Send guest-native ydotool keyboard or pointer input
  launch             Sync and launch development Splinterm on guest Wayland
  stop               Stop the isolated development daemon and test client
  package-build      Build packages from clean committed HEAD inside the guest
  package-status     Inspect guest package and trusted sibling identities
  package-install --confirm-guest-install
                     Install the built guest package, preserving rollback copies
  package-launch     Launch installed /usr/bin siblings with private guest state
  package-stop       Stop installed-package test processes and remove private state
  exec COMMAND...    Run a command from the guest checkout without syncing
  desktop-exec CMD... Run a command in the guest Wayland session
  shell              Open an interactive shell in the guest checkout
USAGE
}

fail() {
  printf 'omarchy-vm: %s\n' "$*" >&2
  exit 1
}

case "$action" in
  -h|--help|help)
    usage
    exit 0
    ;;
esac

[[ $port =~ ^[0-9]+$ ]] || fail 'SPLINTERM_TESTBED_PORT must be numeric'
[[ $user =~ ^[a-z_][a-z0-9_-]*$ ]] || fail 'SPLINTERM_TESTBED_USER is invalid'
remote_parent="/home/$user/Projects"
remote_leaf=${remote_root##*/}
[[ $remote_root == "$remote_parent/$remote_leaf" \
  && $remote_leaf =~ ^splinterm-testbed(-[A-Za-z0-9._]+)?$ ]] \
  || fail "SPLINTERM_TESTBED_REMOTE_ROOT must be $remote_parent/splinterm-testbed[-suffix]"
[[ -n $identity ]] || fail "set SPLINTERM_TESTBED_IDENTITY in $config_file"
[[ -r $identity ]] || fail "identity file is not readable: $identity"
[[ -n $known_hosts ]] || fail "set SPLINTERM_TESTBED_KNOWN_HOSTS in $config_file"
[[ -r $known_hosts ]] || fail "known-hosts file is not readable: $known_hosts"

ssh_options=(
  -i "$identity"
  -p "$port"
  -o BatchMode=yes
  -o IdentitiesOnly=yes
  -o StrictHostKeyChecking=yes
  -o "UserKnownHostsFile=$known_hosts"
  -o ConnectTimeout=10
)
target="$user@$host"

quote_command() {
  local quoted=() argument
  for argument in "$@"; do
    printf -v argument '%q' "$argument"
    quoted+=("$argument")
  done
  printf '%s' "${quoted[*]}"
}

remote_command() {
  local command
  command=$(quote_command "$@")
  ssh "${ssh_options[@]}" "$target" \
    "cd $(printf '%q' "$remote_root") && exec $command"
}

desktop_command() {
  local command
  command=$(quote_command "$@")
  ssh "${ssh_options[@]}" "$target" \
    "set -euo pipefail; instance=\$(hyprctl instances -j | jq -er '.[0]'); runtime=/run/user/\$(id -u); export XDG_RUNTIME_DIR=\$runtime WAYLAND_DISPLAY=\$(jq -r .wl_socket <<<\"\$instance\") HYPRLAND_INSTANCE_SIGNATURE=\$(jq -r .instance <<<\"\$instance\") DBUS_SESSION_BUS_ADDRESS=unix:path=\$runtime/bus; cd $(printf '%q' "$remote_root"); exec $command"
}

input_command() {
  local command
  command=$(quote_command ydotool "$@")
  ssh "${ssh_options[@]}" "$target" \
    "set -euo pipefail; socket=/run/user/\$(id -u)/.ydotool_socket; test -S \"\$socket\"; export YDOTOOL_SOCKET=\$socket; exec $command"
}

sync_checkout() {
  local rsync_rsh
  local rsync_output=()
  printf 'Syncing current worktree to %s:%s…\n' "$target" "$remote_root"
  printf -v rsync_rsh '%q ' ssh "${ssh_options[@]}"
  if [[ ${SPLINTERM_TESTBED_VERBOSE:-0} == 1 ]]; then
    rsync_output=(--human-readable --itemize-changes)
  fi
  ssh "${ssh_options[@]}" "$target" \
    "set -eu; parent=$(printf '%q' "$remote_parent"); root=$(printf '%q' "$remote_root"); mkdir -p \"\$parent\"; test ! -L \"\$parent\"; if test -e \"\$root\" || test -L \"\$root\"; then test -d \"\$root\" && test ! -L \"\$root\"; else mkdir \"\$root\"; fi"
  rsync --archive --delete "${rsync_output[@]}" \
    --exclude=/.env \
    --exclude=/.splinterm-testbed.env \
    --exclude=/.git/ \
    --exclude=/.pi/ \
    --exclude=/.testbed-package/ \
    --exclude=/.worktrees/ \
    --exclude=/benchmark-results/ \
    --exclude=/graphify-out/ \
    --exclude=/packaging/pkg/ \
    --exclude=/packaging/src/ \
    --exclude=/site/node_modules/ \
    --exclude=/site/dist/ \
    --exclude=/target/ \
    --exclude='**/__pycache__/' \
    -e "$rsync_rsh" \
    "$repo_root/" "$target:$remote_root/"
  printf 'Sync complete.\n'
}

prepare_package_checkout() {
  local dirty bundle commit rsync_rsh package_root
  dirty=$(git -C "$repo_root" status --porcelain --untracked-files=normal -- . \
    ':(exclude)AGENTS.md' ':(exclude).splinterm-testbed.env')
  [[ -z $dirty ]] || fail 'package-build requires a clean committed checkout'
  commit=$(git -C "$repo_root" rev-parse --verify HEAD)
  bundle=$(mktemp "${TMPDIR:-/tmp}/splinterm-testbed.XXXXXX.bundle")
  trap 'rm -f "$bundle"' RETURN
  git -C "$repo_root" bundle create "$bundle" HEAD
  package_root="$remote_root/.testbed-package"
  ssh "${ssh_options[@]}" "$target" \
    "set -eu; root=$(printf '%q' "$remote_root"); package=$(printf '%q' "$package_root"); test -d \"\$root\"; test ! -L \"\$root\"; if test -e \"\$package\" || test -L \"\$package\"; then test -d \"\$package\" && test ! -L \"\$package\"; else mkdir \"\$package\"; fi"
  printf -v rsync_rsh '%q ' ssh "${ssh_options[@]}"
  rsync --archive -e "$rsync_rsh" "$bundle" "$target:$package_root/HEAD.bundle"
  ssh "${ssh_options[@]}" "$target" \
    "SPLINTERM_TESTBED_PACKAGE_ROOT=$(printf '%q' "$package_root") SPLINTERM_TESTBED_COMMIT=$(printf '%q' "$commit") bash -s" <<'REMOTE'
set -euo pipefail
cd "$SPLINTERM_TESTBED_PACKAGE_ROOT"
test ! -L HEAD.bundle
rm -rf source.next
trap 'rm -rf source.next' EXIT
git clone --quiet --no-checkout HEAD.bundle source.next
git -C source.next checkout --quiet --detach "$SPLINTERM_TESTBED_COMMIT"
test -z "$(git -C source.next status --porcelain --untracked-files=all)"
rm -rf source
mv source.next source
trap - EXIT
printf 'guest package source: %s (%s)\n' "$SPLINTERM_TESTBED_PACKAGE_ROOT/source" "$SPLINTERM_TESTBED_COMMIT"
REMOTE
  rm -f "$bundle"
  trap - RETURN
}

case "$action" in
  status)
    ssh "${ssh_options[@]}" "$target" \
      "SPLINTERM_TESTBED_ROOT=$(printf '%q' "$remote_root") bash -s" <<'REMOTE'
set -euo pipefail
runtime=/run/user/$(id -u)
export XDG_RUNTIME_DIR="$runtime" DBUS_SESSION_BUS_ADDRESS="unix:path=$runtime/bus"
printf 'host: %s\n' "$(hostname)"
printf 'os: '
sed -n 's/^PRETTY_NAME=//p' /etc/os-release | tr -d '"'
printf 'session: '
loginctl show-session "$(loginctl list-sessions --no-legend | awk '$3 == ENVIRON["USER"] && $4 == "seat0" { print $1; exit }')" \
  -p Active -p Desktop -p Type --value | paste -sd / -
printf 'hyprland: '
hyprctl instances -j | jq -r 'if length == 0 then "not running" else .[0] | "pid=\(.pid) socket=\(.wl_socket) instance=\(.instance)" end'
printf 'rust: '
if command -v rustc >/dev/null; then rustc --version; else printf 'not installed\n'; fi
printf 'cargo: '
if command -v cargo >/dev/null; then cargo --version; else printf 'not installed\n'; fi
printf 'input: '
if command -v ydotool >/dev/null && systemctl --user is-active --quiet ydotool.service \
  && [[ -S $runtime/.ydotool_socket ]]; then
  printf 'ydotool active\n'
else
  printf 'unavailable\n'
fi
printf 'checkout: '
if [[ -f $SPLINTERM_TESTBED_ROOT/Cargo.toml ]]; then
  printf '%s\n' "$SPLINTERM_TESTBED_ROOT"
else
  printf 'not synced (%s)\n' "$SPLINTERM_TESTBED_ROOT"
fi
printf 'space: '
df -h --output=avail "$HOME" | tail -1 | xargs
REMOTE
    ;;
  bootstrap)
    ssh "${ssh_options[@]}" "$target" 'bash -s' <<'REMOTE'
set -euo pipefail
omarchy pkg add rustup ydotool
if ! id -nG | tr ' ' '\n' | grep -Fqx input; then
  printf 'omarchy-vm: guest user must belong to the input group\n' >&2
  exit 1
fi
if [[ -e /dev/uinput && ! -w /dev/uinput ]]; then
  sudo -n chgrp input /dev/uinput
  sudo -n chmod 0660 /dev/uinput
fi
runtime=/run/user/$(id -u)
export XDG_RUNTIME_DIR="$runtime" DBUS_SESSION_BUS_ADDRESS="unix:path=$runtime/bus"
systemctl --user enable --now ydotool.service
for _ in $(seq 1 30); do
  [[ -S $runtime/.ydotool_socket ]] && break
  sleep 0.1
done
[[ -S $runtime/.ydotool_socket ]]
rustup toolchain install 1.88.0 --profile minimal --component clippy,rustfmt
rustup default 1.88.0
rustc --version
cargo --version
REMOTE
    ;;
  sync)
    sync_checkout
    ;;
  check)
    sync_checkout
    printf '\n[1/3] Formatting\n'
    remote_command cargo fmt --all --check
    printf '\n[2/3] Clippy\n'
    remote_command cargo clippy --workspace --all-targets -- -D warnings
    printf '\n[3/3] Workspace tests\n'
    remote_command cargo test --workspace -- --test-threads=1
    printf '\nVM validation complete.\n'
    ;;
  cargo)
    (($# > 0)) || fail 'cargo requires arguments'
    sync_checkout
    remote_command cargo "$@"
    ;;
  ping)
    sync_checkout
    remote_command env "SPLINTERM_REPO=$remote_root" ./splinterm-test ping
    ;;
  qmp)
    [[ -n $qmp_socket ]] || fail "set SPLINTERM_TESTBED_QMP_SOCKET in $config_file"
    (($# > 0)) || fail 'qmp requires a qmp-input.py action'
    "$repo_root/tools/testbed/qmp-input.py" --socket "$qmp_socket" "$@"
    ;;
  input)
    (($# > 0)) || fail 'input requires ydotool arguments'
    input_command "$@"
    ;;
  launch)
    sync_checkout
    ssh "${ssh_options[@]}" "$target" \
      "SPLINTERM_TESTBED_ROOT=$(printf '%q' "$remote_root") bash -s" <<'REMOTE'
set -euo pipefail
instance=$(hyprctl instances -j | jq -er '.[0]')
wayland_display=$(jq -r '.wl_socket' <<<"$instance")
hyprland_signature=$(jq -r '.instance' <<<"$instance")
runtime_dir="/run/user/$(id -u)"
log_dir="$runtime_dir/splinterm-test"
mkdir -p "$log_dir"
cd "$SPLINTERM_TESTBED_ROOT"
XDG_RUNTIME_DIR="$runtime_dir" \
WAYLAND_DISPLAY="$wayland_display" \
HYPRLAND_INSTANCE_SIGNATURE="$hyprland_signature" \
DBUS_SESSION_BUS_ADDRESS="unix:path=$runtime_dir/bus" \
SPLINTERM_REPO="$SPLINTERM_TESTBED_ROOT" \
  nohup ./splinterm-test launch >"$log_dir/client.log" 2>&1 </dev/null &
printf 'guest client pid=%s log=%s\n' "$!" "$log_dir/client.log"
REMOTE
    ;;
  stop)
    remote_command ./splinterm-test stop
    ;;
  package-build)
    (($# == 0)) || fail 'package-build takes no arguments'
    prepare_package_checkout
    package_root="$remote_root/.testbed-package"
    ssh "${ssh_options[@]}" "$target" \
      "cd $(printf '%q' "$package_root/source") && exec ./tools/package/build-local-package.sh"
    ;;
  package-status)
    (($# == 0)) || fail 'package-status takes no arguments'
    ssh "${ssh_options[@]}" "$target" 'bash -s' <<'REMOTE'
set -euo pipefail
printf 'command: %s\n' "$(command -v splinterm 2>/dev/null || printf 'not installed')"
printf 'package: '
pacman -Q splinterm 2>/dev/null || printf 'not installed\n'
printf 'daemon: '
pid=$(pgrep -xo splinterd || true)
if [[ -n $pid ]]; then
  executable=$(readlink -f "/proc/$pid/exe")
  printf 'pid=%s executable=%s\n' "$pid" "$executable"
else
  printf 'not running\n'
fi
if [[ -e /usr/bin/splinterm || -L /usr/bin/splinterm ]]; then
  test -f /usr/bin/splinterm
  test -f /usr/bin/splinterd
  pacman -Qo /usr/bin/splinterm /usr/bin/splinterd
  stat -Lc 'identity: %d:%i %n' /usr/bin/splinterm /usr/bin/splinterd
  desktop-file-validate /usr/share/applications/com.oldjobobo.splinterm.desktop
fi
REMOTE
    ;;
  package-install)
    [[ ${1:-} == --confirm-guest-install && $# == 1 ]] \
      || fail 'package-install requires --confirm-guest-install'
    package_root="$remote_root/.testbed-package"
    ssh "${ssh_options[@]}" "$target" \
      "SPLINTERM_TESTBED_PACKAGE_ROOT=$(printf '%q' "$package_root") bash -s" <<'REMOTE'
set -euo pipefail
package_root=$SPLINTERM_TESTBED_PACKAGE_ROOT
source_root="$package_root/source"
test -d "$source_root/.git"
test ! -L "$package_root"
resolved=$(command -v splinterm 2>/dev/null || true)
[[ -z $resolved || $resolved == /usr/bin/splinterm ]] || {
  printf 'shadowing client would break trusted UI identity: %s\n' "$resolved" >&2
  exit 1
}
if pacman -Q splinterm >/dev/null 2>&1; then
  rollback="$package_root/rollback/$(date -u +%Y%m%dT%H%M%SZ)"
  mkdir -p "$rollback"
  pacman -Q splinterm >"$rollback/package.txt"
  cp --preserve=all /usr/bin/splinterm /usr/bin/splinterd "$rollback/"
  sha256sum /usr/bin/splinterm /usr/bin/splinterd >"$rollback/sha256sums"
  printf 'rollback copies: %s\n' "$rollback"
fi
cd "$source_root"
./tools/package/upgrade-local-package.sh --yes
command -v splinterm | grep -Fx /usr/bin/splinterm
pacman -Qo /usr/bin/splinterm /usr/bin/splinterd
stat -Lc 'identity: %d:%i %n' /usr/bin/splinterm /usr/bin/splinterd
desktop-file-validate /usr/share/applications/com.oldjobobo.splinterm.desktop
runtime="/run/user/$(id -u)/splinterm-package-install-check"
socket="$runtime/splinterd.sock"
state="$package_root/install-check-state"
rm -rf "$runtime" "$state"
mkdir -m 700 "$runtime" "$state"
SPLINTERM_SOCKET="$socket" XDG_STATE_HOME="$state" \
  /usr/bin/splinterd >"$runtime/daemon.log" 2>&1 </dev/null &
daemon_pid=$!
cleanup_install_check() {
  kill "$daemon_pid" 2>/dev/null || true
  wait "$daemon_pid" 2>/dev/null || true
  rm -rf "$runtime" "$state"
}
trap cleanup_install_check EXIT
for _ in $(seq 1 100); do
  if SPLINTERM_SOCKET="$socket" /usr/bin/splinterm ping >/dev/null 2>&1; then break; fi
  kill -0 "$daemon_pid" 2>/dev/null || {
    cat "$runtime/daemon.log" >&2
    exit 1
  }
  sleep 0.05
done
SPLINTERM_SOCKET="$socket" /usr/bin/splinterm ping >/dev/null
SPLINTERM_SOCKET="$socket" /usr/bin/splinterm list >/dev/null
cleanup_install_check
trap - EXIT
REMOTE
    ;;
  package-launch)
    (($# == 0)) || fail 'package-launch takes no arguments'
    package_root="$remote_root/.testbed-package"
    ssh "${ssh_options[@]}" "$target" \
      "SPLINTERM_TESTBED_ROOT=$(printf '%q' "$remote_root") SPLINTERM_TESTBED_PACKAGE_ROOT=$(printf '%q' "$package_root") bash -s" <<'REMOTE'
set -euo pipefail
resolved=$(command -v splinterm 2>/dev/null || true)
[[ $resolved == /usr/bin/splinterm ]]
pacman -Qo /usr/bin/splinterm /usr/bin/splinterd >/dev/null
package_root=$SPLINTERM_TESTBED_PACKAGE_ROOT
test -d "$package_root"
test ! -L "$package_root"
runtime="/run/user/$(id -u)/splinterm-package-test"
socket="$runtime/splinterd.sock"
state="$package_root/acceptance-state"
config="$package_root/acceptance-config"
rm -rf "$runtime" "$state" "$config"
mkdir -m 700 "$runtime" "$state" "$config"
instance=$(hyprctl instances -j | jq -er '.[0]')
wayland_display=$(jq -r '.wl_socket' <<<"$instance")
hyprland_signature=$(jq -r '.instance' <<<"$instance")
export XDG_RUNTIME_DIR="/run/user/$(id -u)"
export DBUS_SESSION_BUS_ADDRESS="unix:path=$XDG_RUNTIME_DIR/bus"
SPLINTERM_SOCKET="$socket" XDG_STATE_HOME="$state" XDG_CONFIG_HOME="$config" \
  nohup /usr/bin/splinterd >"$runtime/daemon.log" 2>&1 </dev/null &
daemon_pid=$!
cleanup_failed_launch() {
  kill "$daemon_pid" 2>/dev/null || true
  wait "$daemon_pid" 2>/dev/null || true
}
trap cleanup_failed_launch ERR
for _ in $(seq 1 100); do
  if SPLINTERM_SOCKET="$socket" /usr/bin/splinterm ping >/dev/null 2>&1; then break; fi
  kill -0 "$daemon_pid" 2>/dev/null || {
    cat "$runtime/daemon.log" >&2
    exit 1
  }
  sleep 0.05
done
SPLINTERM_SOCKET="$socket" /usr/bin/splinterm ping >/dev/null
XDG_RUNTIME_DIR="$XDG_RUNTIME_DIR" \
WAYLAND_DISPLAY="$wayland_display" \
HYPRLAND_INSTANCE_SIGNATURE="$hyprland_signature" \
DBUS_SESSION_BUS_ADDRESS="$DBUS_SESSION_BUS_ADDRESS" \
SPLINTERM_SOCKET="$socket" \
XDG_STATE_HOME="$state" \
XDG_CONFIG_HOME="$config" \
  nohup /usr/bin/splinterm launch --working-directory "$SPLINTERM_TESTBED_ROOT" \
    >"$runtime/client.log" 2>&1 </dev/null &
printf 'guest packaged client pid=%s daemon=%s log=%s\n' "$!" "$daemon_pid" "$runtime/client.log"
trap - ERR
REMOTE
    ;;
  package-stop)
    (($# == 0)) || fail 'package-stop takes no arguments'
    package_root="$remote_root/.testbed-package"
    ssh "${ssh_options[@]}" "$target" \
      "SPLINTERM_TESTBED_PACKAGE_ROOT=$(printf '%q' "$package_root") bash -s" <<'REMOTE'
set -euo pipefail
runtime="/run/user/$(id -u)/splinterm-package-test"
socket="$runtime/splinterd.sock"
stop_matching() {
  local expected=$1 process executable
  for process in /proc/[0-9]*; do
    [[ -r $process/environ ]] || continue
    cat "$process/environ" 2>/dev/null | tr '\0' '\n' \
      | grep -Fqx "SPLINTERM_SOCKET=$socket" || continue
    executable=$(readlink -f "$process/exe" 2>/dev/null || true)
    [[ $executable == "$expected" ]] && kill "${process##*/}" 2>/dev/null || true
  done
}
stop_matching /usr/bin/splinterm
stop_matching /usr/bin/splinterd
for _ in $(seq 1 100); do
  alive=false
  for process in /proc/[0-9]*; do
    [[ -r $process/environ ]] || continue
    if cat "$process/environ" 2>/dev/null | tr '\0' '\n' \
      | grep -Fqx "SPLINTERM_SOCKET=$socket"; then
      executable=$(readlink -f "$process/exe" 2>/dev/null || true)
      case $executable in /usr/bin/splinterm|/usr/bin/splinterd) alive=true ;; esac
    fi
  done
  [[ $alive == false ]] && break
  sleep 0.05
done
[[ $alive == false ]]
rm -rf "$runtime" "$SPLINTERM_TESTBED_PACKAGE_ROOT/acceptance-state" \
  "$SPLINTERM_TESTBED_PACKAGE_ROOT/acceptance-config"
REMOTE
    ;;
  exec)
    (($# > 0)) || fail 'exec requires a command'
    remote_command "$@"
    ;;
  desktop-exec)
    (($# > 0)) || fail 'desktop-exec requires a command'
    desktop_command "$@"
    ;;
  shell)
    ssh -t "${ssh_options[@]}" "$target" \
      "cd $(printf '%q' "$remote_root") && exec \${SHELL:-/bin/bash} -l"
    ;;
  *)
    usage >&2
    fail "unknown action: $action"
    ;;
esac
