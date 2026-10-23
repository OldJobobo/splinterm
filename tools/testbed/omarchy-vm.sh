#!/usr/bin/env bash
# Remote paths and commands are deliberately quoted and expanded on the client.
# shellcheck disable=SC2029
set -euo pipefail

repo_root=$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")/../.." && pwd)
config_file=${SPLINTERM_TESTBED_CONFIG:-"$repo_root/.splinterm-testbed.env"}
# A caller-selected test checkout must not be replaced by the shared config root.
requested_remote_root=${SPLINTERM_TESTBED_REMOTE_ROOT:-}

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
remote_root=${requested_remote_root:-${SPLINTERM_TESTBED_REMOTE_ROOT:-"/home/$user/Projects/splinterm-testbed"}}
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
  package-build [--vm-acceptance-live-handoff]
                     Build packages from clean committed HEAD inside the guest
                     (the flag builds a TEST-ONLY daemon that forces live
                     handoff on, for live-upgrade VM acceptance only)
  package-status     Inspect guest package and trusted sibling identities
  package-install --confirm-guest-install [--vm-acceptance-live-handoff]
                     Install the built guest package, preserving rollback copies
                     (the flag accepts a TEST-ONLY VM acceptance package)
  package-launch     Launch installed /usr/bin siblings with private guest state
  package-config RELPATH | --clear
                     Seed (and, when live, write) a private config file from stdin
  package-launch-extra launch|xdg [ARGS...]
                     Add one guarded packaged window to the live session
  package-close-extra ID
                     Close one additional packaged window and restore focus
  font-fixture invalid-candidate [--install]
                     Build the invalid live-font fixture in the package root
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
((${#remote_leaf} <= 48)) || fail 'SPLINTERM_TESTBED_REMOTE_ROOT leaf exceeds socket path limit'
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
    "set -euo pipefail; instance=\$(hyprctl instances -j | jq -er 'if length == 1 then .[0] else error(\"expected exactly one Hyprland instance\") end'); runtime=/run/user/\$(id -u); export XDG_RUNTIME_DIR=\$runtime WAYLAND_DISPLAY=\$(jq -r .wl_socket <<<\"\$instance\") HYPRLAND_INSTANCE_SIGNATURE=\$(jq -r .instance <<<\"\$instance\") DBUS_SESSION_BUS_ADDRESS=unix:path=\$runtime/bus; cd $(printf '%q' "$remote_root"); exec $command"
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
instance=$(hyprctl instances -j | jq -er \
  'if length == 1 then .[0] else error("expected exactly one Hyprland instance") end')
runtime_dir="/run/user/$(id -u)"
log_dir="$runtime_dir/splinterm-test"
window_state="$log_dir/guest-window.json"
mkdir -m 700 -p "$log_dir"
export XDG_RUNTIME_DIR="$runtime_dir"
export WAYLAND_DISPLAY
WAYLAND_DISPLAY=$(jq -r '.wl_socket' <<<"$instance")
export HYPRLAND_INSTANCE_SIGNATURE
HYPRLAND_INSTANCE_SIGNATURE=$(jq -r '.instance' <<<"$instance")
export DBUS_SESSION_BUS_ADDRESS="unix:path=$runtime_dir/bus"
export YDOTOOL_SOCKET="$runtime_dir/.ydotool_socket"
cd "$SPLINTERM_TESTBED_ROOT"
[[ ! -e $window_state ]] || {
  printf 'development guest window state exists; run stop first\n' >&2
  exit 1
}
# The launcher rebuilds on every run; finish a cold build before the bounded map wait.
case ${SPLINTERM_TEST_PROFILE:-release} in
  release) cargo build -q --release -p splinterd -p splinterm -p splinterm-pty ;;
  debug) cargo build -q -p splinterd -p splinterm -p splinterm-pty ;;
  *) printf 'invalid SPLINTERM_TEST_PROFILE\n' >&2; exit 2 ;;
esac
client_pid=
cleanup_failed_launch() {
  [[ -z $client_pid ]] || kill "$client_pid" 2>/dev/null || true
  SPLINTERM_REPO="$SPLINTERM_TESTBED_ROOT" ./splinterm-test stop || true
  [[ -z $client_pid ]] || wait "$client_pid" 2>/dev/null || true
  [[ ! -e $window_state ]] \
    || python tools/testbed/guest-window.py restore --state "$window_state" || true
}
trap cleanup_failed_launch ERR
python tools/testbed/guest-window.py prepare --state "$window_state"
client_token=$(python -c 'import secrets; print(secrets.token_hex(16))')
SPLINTERM_TEST_WINDOW_TOKEN="$client_token" \
SPLINTERM_REPO="$SPLINTERM_TESTBED_ROOT" \
  nohup ./splinterm-test launch >"$log_dir/client.log" 2>&1 </dev/null &
client_pid=$!
python tools/testbed/guest-window.py place --state "$window_state" \
  --client-pid "$client_pid" --client-token "$client_token"
printf 'guest client pid=%s log=%s\n' "$client_pid" "$log_dir/client.log"
trap - ERR
REMOTE
    ;;
  stop)
    ssh "${ssh_options[@]}" "$target" \
      "SPLINTERM_TESTBED_ROOT=$(printf '%q' "$remote_root") bash -s" <<'REMOTE'
set -euo pipefail
instance=$(hyprctl instances -j | jq -er \
  'if length == 1 then .[0] else error("expected exactly one Hyprland instance") end')
runtime_dir="/run/user/$(id -u)"
window_state="$runtime_dir/splinterm-test/guest-window.json"
export XDG_RUNTIME_DIR="$runtime_dir"
export WAYLAND_DISPLAY
WAYLAND_DISPLAY=$(jq -r '.wl_socket' <<<"$instance")
export HYPRLAND_INSTANCE_SIGNATURE
HYPRLAND_INSTANCE_SIGNATURE=$(jq -r '.instance' <<<"$instance")
export DBUS_SESSION_BUS_ADDRESS="unix:path=$runtime_dir/bus"
export YDOTOOL_SOCKET="$runtime_dir/.ydotool_socket"
cd "$SPLINTERM_TESTBED_ROOT"
./splinterm-test stop
if [[ -e $window_state ]]; then
  python tools/testbed/guest-window.py restore --state "$window_state"
fi
REMOTE
    ;;
  package-build)
    build_arguments=()
    if [[ $# == 1 && $1 == --vm-acceptance-live-handoff ]]; then
      build_arguments=(--vm-acceptance-live-handoff)
      printf 'TEST-ONLY: the guest daemon is a VM acceptance build (VM acceptance only).\n'
    else
      (($# == 0)) || fail 'package-build takes no arguments except --vm-acceptance-live-handoff'
    fi
    prepare_package_checkout
    package_root="$remote_root/.testbed-package"
    ssh "${ssh_options[@]}" "$target" \
      "cd $(printf '%q' "$package_root/source") && exec ./tools/package/build-local-package.sh $(quote_command "${build_arguments[@]}")"
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
  # Plan 0061 M3.17: a TEST-ONLY VM acceptance build carries a marker.
  if pacman -Qi splinterm | grep -Fq 'TEST-ONLY' \
    || grep -aqF 'splinterm-vm-acceptance-live-handoff:' /usr/bin/splinterd; then
    printf 'WARNING: the installed package is a TEST-ONLY VM acceptance build; reinstall a plain build\n'
  fi
fi
REMOTE
    ;;
  package-install)
    install_vm_acceptance=false
    if [[ $# == 2 && ${1:-} == --confirm-guest-install && ${2:-} == --vm-acceptance-live-handoff ]]; then
      install_vm_acceptance=true
    else
      [[ ${1:-} == --confirm-guest-install && $# == 1 ]] \
        || fail 'package-install requires --confirm-guest-install [--vm-acceptance-live-handoff]'
    fi
    package_root="$remote_root/.testbed-package"
    ssh "${ssh_options[@]}" "$target" \
      "SPLINTERM_TESTBED_PACKAGE_ROOT=$(printf '%q' "$package_root") SPLINTERM_TESTBED_RUNTIME_LEAF=$(printf '%q' "$remote_leaf") SPLINTERM_TESTBED_VM_ACCEPTANCE=$install_vm_acceptance bash -s" <<'REMOTE'
set -euo pipefail
package_root=$SPLINTERM_TESTBED_PACKAGE_ROOT
source_root="$package_root/source"
runtime="/run/user/$(id -u)/${SPLINTERM_TESTBED_RUNTIME_LEAF}-package-install-check"
socket="$runtime/splinterd.sock"
state="$package_root/install-check-state"
[[ ! -e $runtime && ! -L $runtime ]] || {
  printf 'guest package install-check runtime already exists: %s\n' "$runtime" >&2
  exit 1
}
[[ ! -e $state && ! -L $state ]] || {
  printf 'guest package install-check state already exists: %s\n' "$state" >&2
  exit 1
}
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
vm_acceptance_arguments=()
if [[ $SPLINTERM_TESTBED_VM_ACCEPTANCE == true ]]; then
  vm_acceptance_arguments=(--allow-vm-acceptance-live-handoff)
  printf 'TEST-ONLY: installing a VM acceptance package.\n'
fi
./tools/package/upgrade-local-package.sh --yes "${vm_acceptance_arguments[@]}"
command -v splinterm | grep -Fx /usr/bin/splinterm
pacman -Qo /usr/bin/splinterm /usr/bin/splinterd
stat -Lc 'identity: %d:%i %n' /usr/bin/splinterm /usr/bin/splinterd
desktop-file-validate /usr/share/applications/com.oldjobobo.splinterm.desktop
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
      "SPLINTERM_TESTBED_ROOT=$(printf '%q' "$remote_root") SPLINTERM_TESTBED_PACKAGE_ROOT=$(printf '%q' "$package_root") SPLINTERM_TESTBED_RUNTIME_LEAF=$(printf '%q' "$remote_leaf") bash -s" <<'REMOTE'
set -euo pipefail
resolved=$(command -v splinterm 2>/dev/null || true)
[[ $resolved == /usr/bin/splinterm ]]
pacman -Qo /usr/bin/splinterm /usr/bin/splinterd >/dev/null
package_root=$SPLINTERM_TESTBED_PACKAGE_ROOT
test -d "$package_root/source/.git"
test ! -L "$package_root"
runtime="/run/user/$(id -u)/${SPLINTERM_TESTBED_RUNTIME_LEAF}-package-test"
socket="$runtime/splinterd.sock"
window_state="$runtime/guest-window.json"
state="$package_root/acceptance-state"
config="$package_root/acceptance-config"
# A live upgrade (Plan 0061) leaves packaged test processes executing a
# replaced inode or a sealed memfd snapshot.
for process in /proc/[0-9]*; do
  [[ -r $process/environ ]] || continue
  cat "$process/environ" 2>/dev/null | tr '\0' '\n' \
    | grep -Fqx "SPLINTERM_SOCKET=$socket" || continue
  executable=$(readlink "$process/exe" 2>/dev/null || true)
  case $executable in
    /usr/bin/splinterm|/usr/bin/splinterd|'/usr/bin/splinterm (deleted)'|'/usr/bin/splinterd (deleted)'|'/memfd:splinterd-client (deleted)'|'/memfd:splinterd-daemon (deleted)')
      printf 'packaged test is already active; run package-stop first\n' >&2
      exit 1
      ;;
  esac
done
[[ ! -e $runtime && ! -L $runtime ]] || {
  printf 'packaged guest runtime exists; inspect its owner before cleanup: %s\n' "$runtime" >&2
  exit 1
}
for path in "$state" "$config"; do
  [[ ! -e $path && ! -L $path ]] || {
    printf 'packaged guest state exists; inspect its owner before cleanup: %s\n' "$path" >&2
    exit 1
  }
done
daemon_pid=
client_pid=
created_runtime=false
created_state=false
created_config=false
cleanup_failed_launch() {
  [[ -z $client_pid ]] || kill "$client_pid" 2>/dev/null || true
  [[ -z $daemon_pid ]] || kill "$daemon_pid" 2>/dev/null || true
  [[ -z $client_pid ]] || wait "$client_pid" 2>/dev/null || true
  [[ -z $daemon_pid ]] || wait "$daemon_pid" 2>/dev/null || true
  if [[ $created_runtime == true && -e $window_state ]]; then
    if ! python "$package_root/source/tools/testbed/guest-window.py" restore \
      --state "$window_state"; then
      printf 'guest restoration failed; private state retained for package-stop: %s\n' \
        "$runtime" >&2
      return 1
    fi
  fi
  # The preflight rejects existing paths; remove only directories this launch made.
  [[ $created_config == false ]] || rm -rf -- "$config"
  [[ $created_state == false ]] || rm -rf -- "$state"
  [[ $created_runtime == false ]] || rm -rf -- "$runtime"
}
trap cleanup_failed_launch ERR
mkdir -m 700 "$runtime"
created_runtime=true
mkdir -m 700 "$state"
created_state=true
mkdir -m 700 "$config"
created_config=true
# A seeded private configuration (package-config) is copied in before launch.
seed="$package_root/acceptance-seed"
if [[ -d $seed && ! -L $seed ]]; then
  cp -R -- "$seed/." "$config/"
fi
# The daemon reads policy only from SPLINTERM_POLICY, so point it at the
# private config's policy.json, which package-config writes and a SIGHUP
# reloads. An empty deny-all policy stands in until one is seeded, so the
# daemon starts deny-all without a missing-file diagnostic.
policy="$config/splinterm/policy.json"
if [[ ! -e $policy ]]; then
  mkdir -p -m 700 -- "$config/splinterm"
  (umask 077 && printf '{"schema":"splinterm.policy.v2","rules":[]}\n' >"$policy")
fi
printf '%s\n' "$package_root" >"$runtime/testbed-root"
instance=$(hyprctl instances -j | jq -er \
  'if length == 1 then .[0] else error("expected exactly one Hyprland instance") end')
export XDG_RUNTIME_DIR="/run/user/$(id -u)"
export WAYLAND_DISPLAY
WAYLAND_DISPLAY=$(jq -r '.wl_socket' <<<"$instance")
export HYPRLAND_INSTANCE_SIGNATURE
HYPRLAND_INSTANCE_SIGNATURE=$(jq -r '.instance' <<<"$instance")
export DBUS_SESSION_BUS_ADDRESS="unix:path=$XDG_RUNTIME_DIR/bus"
export YDOTOOL_SOCKET="$XDG_RUNTIME_DIR/.ydotool_socket"
python "$package_root/source/tools/testbed/guest-window.py" prepare --state "$window_state"
# Without RUST_LOG the packaged daemon logs only errors; VM acceptance reads
# its info and warn lines (the TEST-ONLY banner, handoff phases).
RUST_LOG=info SPLINTERM_POLICY="$policy" \
  SPLINTERM_SOCKET="$socket" XDG_STATE_HOME="$state" XDG_CONFIG_HOME="$config" \
  nohup /usr/bin/splinterd >"$runtime/daemon.log" 2>&1 </dev/null &
daemon_pid=$!
for _ in $(seq 1 100); do
  if SPLINTERM_SOCKET="$socket" /usr/bin/splinterm ping >/dev/null 2>&1; then break; fi
  kill -0 "$daemon_pid" 2>/dev/null || {
    cat "$runtime/daemon.log" >&2
    exit 1
  }
  sleep 0.05
done
SPLINTERM_SOCKET="$socket" /usr/bin/splinterm ping >/dev/null
client_token=$(python -c 'import secrets; print(secrets.token_hex(16))')
SPLINTERM_TEST_WINDOW_TOKEN="$client_token" \
SPLINTERM_SOCKET="$socket" XDG_STATE_HOME="$state" XDG_CONFIG_HOME="$config" \
  nohup /usr/bin/splinterm launch --working-directory "$SPLINTERM_TESTBED_ROOT" \
    >"$runtime/client.log" 2>&1 </dev/null &
client_pid=$!
python "$package_root/source/tools/testbed/guest-window.py" place --state "$window_state" \
  --client-pid "$client_pid" --client-token "$client_token"
printf 'guest packaged client pid=%s daemon=%s log=%s\n' \
  "$client_pid" "$daemon_pid" "$runtime/client.log"
trap - ERR
REMOTE
    ;;
  package-stop)
    (($# == 0)) || fail 'package-stop takes no arguments'
    package_root="$remote_root/.testbed-package"
    ssh "${ssh_options[@]}" "$target" \
      "SPLINTERM_TESTBED_PACKAGE_ROOT=$(printf '%q' "$package_root") SPLINTERM_TESTBED_RUNTIME_LEAF=$(printf '%q' "$remote_leaf") bash -s" <<'REMOTE'
set -euo pipefail
runtime="/run/user/$(id -u)/${SPLINTERM_TESTBED_RUNTIME_LEAF}-package-test"
socket="$runtime/splinterd.sock"
window_state="$runtime/guest-window.json"
[[ -d $runtime && ! -L $runtime ]] || {
  printf 'no owned packaged guest runtime: %s\n' "$runtime" >&2
  exit 1
}
owner="$runtime/testbed-root"
[[ -f $owner && ! -L $owner \
  && $(<"$owner") == "$SPLINTERM_TESTBED_PACKAGE_ROOT" ]] || {
  printf 'packaged guest runtime owner mismatch: %s\n' "$runtime" >&2
  exit 1
}
# A live upgrade (Plan 0061) leaves packaged test processes executing a
# replaced inode ("/usr/bin/... (deleted)") or a sealed memfd snapshot (the
# adopted daemon and relaunched Windows), still bound to the private socket.
packaged_role() {
  case $1 in
    /usr/bin/splinterm|'/usr/bin/splinterm (deleted)'|'/memfd:splinterd-client (deleted)') printf client ;;
    /usr/bin/splinterd|'/usr/bin/splinterd (deleted)'|'/memfd:splinterd-daemon (deleted)') printf daemon ;;
    *) return 1 ;;
  esac
}
stop_matching() {
  local expected=$1 process executable
  for process in /proc/[0-9]*; do
    [[ -r $process/environ ]] || continue
    cat "$process/environ" 2>/dev/null | tr '\0' '\n' \
      | grep -Fqx "SPLINTERM_SOCKET=$socket" || continue
    executable=$(readlink "$process/exe" 2>/dev/null || true)
    [[ $(packaged_role "$executable") == "$expected" ]] && kill "${process##*/}" 2>/dev/null || true
  done
}
stop_matching client
stop_matching daemon
for _ in $(seq 1 100); do
  alive=false
  for process in /proc/[0-9]*; do
    [[ -r $process/environ ]] || continue
    if cat "$process/environ" 2>/dev/null | tr '\0' '\n' \
      | grep -Fqx "SPLINTERM_SOCKET=$socket"; then
      executable=$(readlink "$process/exe" 2>/dev/null || true)
      packaged_role "$executable" >/dev/null && alive=true
    fi
  done
  [[ $alive == false ]] && break
  sleep 0.05
done
[[ $alive == false ]]
if [[ -e $window_state ]]; then
  instance=$(hyprctl instances -j | jq -er \
    'if length == 1 then .[0] else error("expected exactly one Hyprland instance") end')
  export XDG_RUNTIME_DIR="/run/user/$(id -u)"
  export WAYLAND_DISPLAY
  WAYLAND_DISPLAY=$(jq -r '.wl_socket' <<<"$instance")
  export HYPRLAND_INSTANCE_SIGNATURE
  HYPRLAND_INSTANCE_SIGNATURE=$(jq -r '.instance' <<<"$instance")
  export DBUS_SESSION_BUS_ADDRESS="unix:path=$XDG_RUNTIME_DIR/bus"
  export YDOTOOL_SOCKET="$XDG_RUNTIME_DIR/.ydotool_socket"
  python "$SPLINTERM_TESTBED_PACKAGE_ROOT/source/tools/testbed/guest-window.py" restore \
    --state "$window_state"
fi
rm -rf "$runtime" "$SPLINTERM_TESTBED_PACKAGE_ROOT/acceptance-state" \
  "$SPLINTERM_TESTBED_PACKAGE_ROOT/acceptance-config" \
  "$SPLINTERM_TESTBED_PACKAGE_ROOT/font-fixtures"
REMOTE
    ;;
  package-config)
    package_root="$remote_root/.testbed-package"
    if [[ ${1:-} == --clear && $# == 1 ]]; then
      ssh "${ssh_options[@]}" "$target" \
        "SPLINTERM_TESTBED_PACKAGE_ROOT=$(printf '%q' "$package_root") bash -s" <<'REMOTE'
set -euo pipefail
seed="$SPLINTERM_TESTBED_PACKAGE_ROOT/acceptance-seed"
test ! -L "$SPLINTERM_TESTBED_PACKAGE_ROOT"
[[ ! -L $seed ]] || { printf 'config seed is a symlink: %s\n' "$seed" >&2; exit 1; }
rm -rf -- "$seed"
printf 'cleared private config seed: %s\n' "$seed"
REMOTE
      exit 0
    fi
    (($# == 1)) || fail 'package-config takes RELPATH (content on stdin) or --clear'
    relpath=$1
    # Match bytes, not locale characters, so non-ASCII names never pass.
    LC_ALL=C
    [[ ${#relpath} -le 128 \
      && $relpath =~ ^(splinterm|fontconfig)(/[A-Za-z0-9_][A-Za-z0-9._-]*)+$ \
      && ! $relpath =~ (^|/)\.\.?(/|$) ]] \
      || fail 'package-config RELPATH must be splinterm/... or fontconfig/... without dot segments'
    # The remote script travels as an argument so stdin carries the content.
    remote_script=$(cat <<'REMOTE'
set -euo pipefail
package_root=$SPLINTERM_TESTBED_PACKAGE_ROOT
relpath=$SPLINTERM_TESTBED_CONFIG_PATH
runtime="/run/user/$(id -u)/${SPLINTERM_TESTBED_RUNTIME_LEAF}-package-test"
seed="$package_root/acceptance-seed"
config="$package_root/acceptance-config"
test -d "$package_root" && test ! -L "$package_root"
[[ ! -L $seed ]] || { printf 'config seed is a symlink: %s\n' "$seed" >&2; exit 1; }
content=$(mktemp "$package_root/.config-content.XXXXXX")
trap 'rm -f -- "$content"' EXIT
head -c 65537 >"$content"
(( $(stat -c %s "$content") <= 65536 )) || {
  printf 'package-config content exceeds 64 KiB\n' >&2
  exit 1
}
write_into() {
  local root=$1 destination
  destination="$root/$relpath"
  mkdir -p -m 700 -- "$(dirname -- "$destination")"
  [[ ! -L $destination ]] || { printf 'refusing symlink: %s\n' "$destination" >&2; exit 1; }
  # Replace atomically: a watcher polling the file never sees it missing.
  staged=$(mktemp "$(dirname -- "$destination")/.package-config.XXXXXX")
  install -m 600 -- "$content" "$staged"
  mv -f -- "$staged" "$destination"
  printf 'wrote %s\n' "$destination"
}
mkdir -p -m 700 -- "$seed"
write_into "$seed"
# A live packaged session owned by this checkout also receives the file now.
owner="$runtime/testbed-root"
if [[ -d $runtime && ! -L $runtime && -f $owner && ! -L $owner \
  && $(<"$owner") == "$package_root" && -d $config && ! -L $config ]]; then
  write_into "$config"
fi
REMOTE
)
    ssh "${ssh_options[@]}" "$target" \
      "SPLINTERM_TESTBED_PACKAGE_ROOT=$(printf '%q' "$package_root") SPLINTERM_TESTBED_RUNTIME_LEAF=$(printf '%q' "$remote_leaf") SPLINTERM_TESTBED_CONFIG_PATH=$(printf '%q' "$relpath") bash -c $(printf '%q' "$remote_script")"
    ;;
  package-launch-extra)
    (($# >= 1)) || fail 'package-launch-extra requires launch|xdg [ARGS...]'
    case $1 in
      launch|xdg) extra_program=$1 ;;
      *) fail 'package-launch-extra requires launch|xdg [ARGS...]' ;;
    esac
    shift
    package_root="$remote_root/.testbed-package"
    extra_arguments=$(quote_command "$@")
    ssh "${ssh_options[@]}" "$target" \
      "SPLINTERM_TESTBED_PACKAGE_ROOT=$(printf '%q' "$package_root") SPLINTERM_TESTBED_RUNTIME_LEAF=$(printf '%q' "$remote_leaf") SPLINTERM_TESTBED_EXTRA_PROGRAM=$extra_program SPLINTERM_TESTBED_EXTRA_ARGUMENTS=$(printf '%q' "$extra_arguments") bash -s" <<'REMOTE'
set -euo pipefail
package_root=$SPLINTERM_TESTBED_PACKAGE_ROOT
runtime="/run/user/$(id -u)/${SPLINTERM_TESTBED_RUNTIME_LEAF}-package-test"
socket="$runtime/splinterd.sock"
window_state="$runtime/guest-window.json"
state="$package_root/acceptance-state"
config="$package_root/acceptance-config"
owner="$runtime/testbed-root"
[[ -d $runtime && ! -L $runtime && -f $owner && ! -L $owner \
  && $(<"$owner") == "$package_root" ]] || {
  printf 'no packaged session owned by this checkout; run package-launch first\n' >&2
  exit 1
}
[[ -S $socket && -f $window_state && -d $state && -d $config ]] || {
  printf 'packaged session is incomplete: %s\n' "$runtime" >&2
  exit 1
}
eval "extra_arguments=($SPLINTERM_TESTBED_EXTRA_ARGUMENTS)"
case $SPLINTERM_TESTBED_EXTRA_PROGRAM in
  launch) program=(/usr/bin/splinterm launch) ;;
  xdg) program=(/usr/bin/splinterm-xdg-terminal-exec) ;;
  *) printf 'invalid extra program\n' >&2; exit 2 ;;
esac
# Locate the compositor before claiming a slot, so a failure here leaks nothing.
instance=$(hyprctl instances -j | jq -er \
  'if length == 1 then .[0] else error("expected exactly one Hyprland instance") end')
export XDG_RUNTIME_DIR="/run/user/$(id -u)"
export WAYLAND_DISPLAY
WAYLAND_DISPLAY=$(jq -r '.wl_socket' <<<"$instance")
export HYPRLAND_INSTANCE_SIGNATURE
HYPRLAND_INSTANCE_SIGNATURE=$(jq -r '.instance' <<<"$instance")
export DBUS_SESSION_BUS_ADDRESS="unix:path=$XDG_RUNTIME_DIR/bus"
extra=
extra_state=
client_pid=
cleanup_failed_extra() {
  [[ -n $extra ]] || return 0
  [[ -z $client_pid ]] || kill "$client_pid" 2>/dev/null || true
  [[ -z $client_pid ]] || wait "$client_pid" 2>/dev/null || true
  if [[ -e $extra_state ]]; then
    if ! python "$package_root/source/tools/testbed/guest-window.py" restore \
      --state "$extra_state"; then
      printf 'additional window restoration failed; state retained: %s\n' "$extra" >&2
      return 1
    fi
  fi
  rm -rf -- "$extra"
}
# Armed before the slot is claimed: any later failure releases the slot.
trap cleanup_failed_extra ERR
for index in 1 2 3 4; do
  if mkdir -m 700 "$runtime/extra-$index" 2>/dev/null; then
    extra="$runtime/extra-$index"
    break
  fi
done
[[ -n $extra ]] || { printf 'at most four additional windows per session\n' >&2; exit 1; }
extra_state="$extra/guest-window.json"
python "$package_root/source/tools/testbed/guest-window.py" prepare \
  --state "$extra_state" --owner-state "$window_state"
client_token=$(python -c 'import secrets; print(secrets.token_hex(16))')
printf '%s\n' "$client_token" >"$extra/client-token"
SPLINTERM_TEST_WINDOW_TOKEN="$client_token" \
SPLINTERM_SOCKET="$socket" XDG_STATE_HOME="$state" XDG_CONFIG_HOME="$config" \
  nohup "${program[@]}" "${extra_arguments[@]}" >"$extra/client.log" 2>&1 </dev/null &
client_pid=$!
printf '%s\n' "$client_pid" >"$extra/client-pid"
python "$package_root/source/tools/testbed/guest-window.py" place --state "$extra_state" \
  --client-pid "$client_pid" --client-token "$client_token"
printf 'additional guest window id=%s pid=%s log=%s\n' \
  "${extra##*-}" "$client_pid" "$extra/client.log"
trap - ERR
REMOTE
    ;;
  package-close-extra)
    [[ $# == 1 && ${1:-} =~ ^[1-4]$ ]] || fail 'package-close-extra requires an id from 1 to 4'
    package_root="$remote_root/.testbed-package"
    ssh "${ssh_options[@]}" "$target" \
      "SPLINTERM_TESTBED_PACKAGE_ROOT=$(printf '%q' "$package_root") SPLINTERM_TESTBED_RUNTIME_LEAF=$(printf '%q' "$remote_leaf") SPLINTERM_TESTBED_EXTRA_ID=$1 bash -s" <<'REMOTE'
set -euo pipefail
package_root=$SPLINTERM_TESTBED_PACKAGE_ROOT
runtime="/run/user/$(id -u)/${SPLINTERM_TESTBED_RUNTIME_LEAF}-package-test"
socket="$runtime/splinterd.sock"
owner="$runtime/testbed-root"
extra="$runtime/extra-$SPLINTERM_TESTBED_EXTRA_ID"
[[ -d $runtime && ! -L $runtime && -f $owner && ! -L $owner \
  && $(<"$owner") == "$package_root" ]] || {
  printf 'no packaged session owned by this checkout\n' >&2
  exit 1
}
[[ -d $extra && ! -L $extra ]] || { printf 'no additional window %s\n' "$extra" >&2; exit 1; }
token=
if [[ -f $extra/client-token && ! -L $extra/client-token ]]; then
  token=$(<"$extra/client-token")
fi
# A launch that failed before writing its token started no process; still
# restore and release the slot. Stop only processes carrying this window's
# launch token and private socket.
if [[ $token =~ ^[0-9a-f]{32}$ ]]; then
  for process in /proc/[0-9]*; do
    [[ -r $process/environ ]] || continue
    environment=$(tr '\0' '\n' <"$process/environ" 2>/dev/null || true)
    grep -Fqx "SPLINTERM_TEST_WINDOW_TOKEN=$token" <<<"$environment" || continue
    grep -Fqx "SPLINTERM_SOCKET=$socket" <<<"$environment" || continue
    kill "${process##*/}" 2>/dev/null || true
  done
elif [[ -n $token ]]; then
  printf 'additional window token is malformed; no process was stopped\n' >&2
fi
instance=$(hyprctl instances -j | jq -er \
  'if length == 1 then .[0] else error("expected exactly one Hyprland instance") end')
export XDG_RUNTIME_DIR="/run/user/$(id -u)"
export WAYLAND_DISPLAY
WAYLAND_DISPLAY=$(jq -r '.wl_socket' <<<"$instance")
export HYPRLAND_INSTANCE_SIGNATURE
HYPRLAND_INSTANCE_SIGNATURE=$(jq -r '.instance' <<<"$instance")
export DBUS_SESSION_BUS_ADDRESS="unix:path=$XDG_RUNTIME_DIR/bus"
if [[ -e $extra/guest-window.json ]]; then
  python "$package_root/source/tools/testbed/guest-window.py" restore \
    --state "$extra/guest-window.json"
fi
rm -rf -- "$extra"
REMOTE
    ;;
  font-fixture)
    [[ ${1:-} == invalid-candidate && ($# == 1 || ($# == 2 && ${2:-} == --install)) ]] \
      || fail 'font-fixture requires invalid-candidate [--install]'
    install_fixture=false
    [[ ${2:-} != --install ]] || install_fixture=true
    package_root="$remote_root/.testbed-package"
    ssh "${ssh_options[@]}" "$target" \
      "SPLINTERM_TESTBED_PACKAGE_ROOT=$(printf '%q' "$package_root") SPLINTERM_TESTBED_RUNTIME_LEAF=$(printf '%q' "$remote_leaf") SPLINTERM_TESTBED_INSTALL_FIXTURE=$install_fixture bash -s" <<'REMOTE'
set -euo pipefail
package_root=$SPLINTERM_TESTBED_PACKAGE_ROOT
runtime="/run/user/$(id -u)/${SPLINTERM_TESTBED_RUNTIME_LEAF}-package-test"
fixtures="$package_root/font-fixtures"
config="$package_root/acceptance-config"
destination="$config/fontconfig/fonts.conf"
test -d "$package_root/source/.git" && test ! -L "$package_root"
[[ ! -L $fixtures ]] || { printf 'font fixture root is a symlink\n' >&2; exit 1; }
mkdir -p -m 700 -- "$fixtures"
# Every build gets a fresh generation directory and is never moved: the
# fixture's fonts.conf and private cache name their own absolute paths. The
# generation a live fonts.conf points at is left untouched until a validated
# replacement is installed, and a failed build discards only its own directory.
generation=$(mktemp -d "$fixtures/invalid-candidate.XXXXXX")
output="$generation/fixture"
built=false
trap '[[ $built == true ]] || rm -rf -- "$generation"' EXIT
python "$package_root/source/tools/testbed/font-fixture.py" invalid-candidate \
  --output "$output"
if [[ $SPLINTERM_TESTBED_INSTALL_FIXTURE == true ]]; then
  owner="$runtime/testbed-root"
  [[ -d $runtime && ! -L $runtime && -f $owner && ! -L $owner \
    && $(<"$owner") == "$package_root" && -d $config && ! -L $config ]] || {
    printf 'no packaged session owned by this checkout to install into\n' >&2
    exit 1
  }
  mkdir -p -m 700 -- "$config/fontconfig"
  [[ ! -L $destination ]] || { printf 'refusing symlink: %s\n' "$destination" >&2; exit 1; }
  if [[ -e $destination && ! -e $destination.before-fixture ]]; then
    cp -- "$destination" "$destination.before-fixture"
  fi
  # Replace atomically: the live font watcher never sees fonts.conf missing.
  staged=$(mktemp "$config/fontconfig/.fonts.conf.XXXXXX")
  install -m 600 -- "$output/fonts.conf" "$staged"
  mv -f -- "$staged" "$destination"
  printf 'installed invalid-candidate fonts.conf into %s\n' "$destination"
fi
built=true
# Prune older generations that no live fonts.conf references.
for previous in "$fixtures"/invalid-candidate.*; do
  [[ -d $previous && ! -L $previous && $previous != "$generation" ]] || continue
  if [[ -f $destination ]] && grep -Fq -- "$previous/" "$destination"; then
    continue
  fi
  rm -rf -- "$previous"
done
printf 'invalid-candidate fixture built in %s\n' "$output"
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
