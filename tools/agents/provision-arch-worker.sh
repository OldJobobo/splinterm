#!/usr/bin/env bash
# Deliberate Arch worker bootstrap. No downloads, credential copies, or sudo grants.
set -euo pipefail
umask 077
export PATH=/usr/bin:/bin

fail() { printf 'Worker setup: %s\n' "$*" >&2; exit 1; }
usage() {
  printf 'Usage: %s --check|--apply HOST OPERATOR RELEASE_ARCHIVE CONTROLLER_PUBLIC_KEY\n' "${0##*/}" >&2
  exit 2
}
[[ $# == 5 ]] || usage
mode=$1 expected_host=$2 operator=$3 archive=$4 key_file=$5
[[ $mode == --check || $mode == --apply ]] || usage
[[ $mode != --apply || $EUID == 0 ]] || fail 'Apply requires normal interactive sudo.'
[[ $mode != --check || $EUID != 0 ]] || fail 'Run the check as the operator, not root.'
IFS= read -r actual_host < /proc/sys/kernel/hostname
[[ $actual_host == "$expected_host" ]] || fail 'Host mismatch; nothing was changed.'
[[ $(uname -m) == x86_64 ]] || fail 'This pinned release is for x86_64 Linux only.'
[[ $operator =~ ^[a-z_][a-z0-9_-]*$ ]] || fail 'Invalid operator account name.'

account=splinterm-worker
home=/home/splinterm-worker
runtime_root=/usr/local/lib/splinterm-agent
runtime=$runtime_root/pi-1.3.0
archive_sha=24f5be328b66d4da36876296989501974f43ab8434fbb57c35c523a3ea505ded
binary_sha=c2415d6e6f7c67b0a55744c5ace5054936e252fcc3cf56ce087bc314061315ca

for tool in sha256sum tar mktemp install cp chmod chown find stat getent flock useradd loginctl runuser sshd ssh-keygen git python3 node fd rg cargo rustc bwrap sudo; do
  command -v "$tool" >/dev/null || fail "Missing system prerequisite: $tool"
done
if [[ $mode == --apply ]]; then
  exec 9>/run/lock/splinterm-worker-provision.lock
  flock -n 9 || fail 'Another worker setup holds the lock.'
fi
operator_uid=$(id -u "$operator") || fail 'Operator account does not exist.'
[[ $operator_uid -ge 1000 ]] || fail 'Operator must be a normal non-root account.'
operator_home=$(getent passwd "$operator" | cut -d: -f6)
[[ $operator_home == /* && -d $operator_home && ! -L $operator_home ]] || fail 'Operator home is missing or symlinked.'
[[ $(stat -c '%a:%u' "$operator_home") == "700:$operator_uid" ]] || fail 'Operator home must be operator-owned and private (0700); do not change it automatically.'
getent passwd "$account" >/dev/null && fail 'Worker account exists; inspect it instead of rerunning setup.'
getent group "$account" >/dev/null && fail 'Worker group exists; inspect it instead of rerunning setup.'
for path in "$home" "$runtime_root"; do
  [[ ! -e $path && ! -L $path ]] || fail "Destination already exists: $path"
done
[[ -f $archive && ! -L $archive ]] || fail 'Release archive must be a regular, non-symlink file.'
[[ -f $key_file && ! -L $key_file ]] || fail 'Controller public key must be a regular, non-symlink file.'
controller_key=$(<"$key_file")
[[ $controller_key == ssh-ed25519\ * && $controller_key != *$'\n'* && $controller_key != *$'\r'* ]] || fail 'Use one plain Ed25519 public key, without options or extra lines.'
printf '%s\n' "$controller_key" | ssh-keygen -lf - >/dev/null || fail 'Controller public key is invalid.'
actual=$(sha256sum -- "$archive")
[[ ${actual%% *} == "$archive_sha" ]] || fail 'Release archive checksum does not match pinned Pi 1.3.0.'

if [[ $mode == --apply ]]; then
  ssh_config=$(sshd -T) || fail 'Cannot inspect effective sshd configuration.'
  printf '%s\n' "$ssh_config" | grep -Eix '[[:space:]]*usepam[[:space:]]+yes[[:space:]]*' >/dev/null || fail 'UsePAM is not enabled; no SSH policy will be changed automatically.'
  stage=$(mktemp -d /run/splinterm-worker-provision.XXXXXX)
else
  stage=$(mktemp -d /tmp/splinterm-worker-preflight.XXXXXX)
fi
complete=false
cleanup() {
  if [[ $complete == true ]]; then
    # Only the exact private directory created by this invocation is disposable.
    rm -rf -- "${stage:?}"
  else
    printf 'Stopped; preserve partial state for inspection. Private staging: %s\n' "$stage" >&2
  fi
}
trap cleanup EXIT
install -m 0600 -- "$archive" "$stage/release.tar.gz"
actual=$(sha256sum "$stage/release.tar.gz")
[[ ${actual%% *} == "$archive_sha" ]] || fail 'Copied archive checksum mismatch.'
mkdir "$stage/runtime"
tar --extract --gzip --no-same-owner --no-same-permissions --strip-components=1 \
  --file="$stage/release.tar.gz" --directory="$stage/runtime"
[[ -z $(find "$stage/runtime" ! -type d ! -type f -print -quit) ]] || fail 'Unexpected non-regular release entry.'
actual=$(sha256sum "$stage/runtime/pi")
[[ ${actual%% *} == "$binary_sha" ]] || fail 'Extracted Pi binary checksum mismatch.'

if [[ $mode == --check ]]; then
  mkdir -m 0700 "$stage/probe-home"
  version=$(env -i HOME="$stage/probe-home" USER="$operator" LOGNAME="$operator" \
    PATH=/usr/bin:/bin PI_OFFLINE=1 "$stage/runtime/pi" --version)
  [[ $version == 1.3.0 ]] || fail 'Pinned Pi cannot run on this host.'
  complete=true
  printf '%s\n' 'Ready for operator sudo: pinned Pi runs; no account, runtime, SSH policy, credentials, or services were changed.'
  exit 0
fi

# Everything below is an explicit root apply. Pi itself is never run as root.
install -d -m 0755 -o root -g root "$runtime_root"
cp -R --no-preserve=ownership -- "$stage/runtime" "$runtime"
find "$runtime" -type d -exec chmod 0755 {} +
find "$runtime" -type f -perm /111 -exec chmod 0755 {} +
find "$runtime" -type f ! -perm /111 -exec chmod 0644 {} +
actual=$(sha256sum "$runtime/pi")
[[ ${actual%% *} == "$binary_sha" ]] || fail 'Installed binary checksum mismatch.'
useradd --no-create-home --no-log-init --user-group --groups '' \
  --home-dir "$home" --shell /bin/bash -K UMASK=077 "$account"
install -d -m 0700 -o "$account" -g "$account" \
  "$home" "$home/.ssh" "$home/.pi" "$home/.pi/agent"
printf 'restrict %s\n' "$controller_key" > "$home/.ssh/authorized_keys"
printf '%s\n' '{"defaultProvider":"openai","defaultModel":"gpt-6.1-sol","defaultProjectTrust":"never","packages":[]}' > "$home/.pi/agent/settings.json"
chown "$account:$account" "$home/.ssh/authorized_keys" "$home/.pi/agent/settings.json"
chmod 0600 "$home/.ssh/authorized_keys" "$home/.pi/agent/settings.json"
loginctl enable-linger "$account"
version=$(runuser -u "$account" -- env -i HOME="$home" USER="$account" LOGNAME="$account" \
  PATH=/usr/bin:/bin PI_OFFLINE=1 "$runtime/pi" --version)
[[ $version == 1.3.0 ]] || fail 'Worker cannot run the pinned Pi.'
[[ $(id -Gn "$account") == "$account" ]] || fail 'Worker has unexpected supplementary groups.'
runuser -u "$account" -- test ! -x "$operator_home" || fail 'Worker can traverse the operator home.'
runuser -u "$account" -- test ! -r "$operator_home/.pi/agent/auth.json" || fail 'Worker can read operator credentials.'
runuser -u "$account" -- test ! -w /var/run/docker.sock || fail 'Worker can access the Docker socket.'
if runuser -u "$account" -- sudo -n true >/dev/null 2>&1 ||
  runuser -u "$account" -- sudo -n -l >/dev/null 2>&1 ||
  runuser -u "$account" -- sudo -n /usr/bin/pacman --version >/dev/null 2>&1; then
  fail 'Worker unexpectedly has sudo authority; inspect before use.'
fi
[[ ! -e $home/.pi/agent/auth.json ]] || fail 'Unexpected credentials in the new worker home.'
complete=true
printf '%s\n' 'Worker account and pinned runtime installed. No provider login or model job was performed.'
id "$account"
loginctl show-user "$account" -p Linger
printf '%s\n' 'Next: coordinator verifies the account, then operator logs Pi in under the worker UID.'
