#!/usr/bin/env bash
set -euo pipefail

root=$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)
repository=ministry-of-taste/splinterm
assume_yes=false
source_build=false
run_checks=false

usage() {
  cat <<'EOF'
Usage: ./install.sh [--source] [--check] [-y|--yes]

Download and install the newest published Splinterm versioned release on x86_64
Arch/Omarchy without compiling locally.

  --source     Build and install the current committed checkout instead
  --check      Source mode: run the full package test suite (implies --source)
  -y, --yes    Skip dependency and installation confirmations
  -h, --help   Show this help

The default release installer uses an authenticated GitHub CLI session when
available and otherwise uses anonymous public release downloads. It never opts
a fresh installation into the optional MCP package; an
existing MCP installation is upgraded to keep package versions matched.
EOF
}

while (($#)); do
  case $1 in
    --source) source_build=true ;;
    --check) source_build=true; run_checks=true ;;
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
for command in sudo python; do
  command -v "$command" >/dev/null 2>&1 || {
    printf 'Required command not found: %s\n' "$command" >&2
    exit 1
  }
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

confirm() {
  local prompt=$1
  if [[ $assume_yes == true ]]; then
    return 0
  fi
  if command -v gum >/dev/null 2>&1; then
    gum confirm "$prompt"
  elif [[ -t 0 ]]; then
    local answer
    read -r -p "$prompt [y/N] " answer
    [[ $answer == [yY] || $answer == [yY][eE][sS] ]]
  else
    printf 'confirmation requires a terminal; rerun with --yes\n' >&2
    return 1
  fi
}

install_dependencies() {
  local -a required=("$@")
  local missing
  missing=$(pacman -T "${required[@]}" 2>/dev/null || true)
  if [[ -z $missing ]]; then
    return 0
  fi
  printf 'Missing package dependencies:\n%s\n' "$missing"
  confirm 'Install the missing dependencies?' || exit 0
  local -a missing_packages pacman_args=(-S --needed)
  mapfile -t missing_packages <<<"$missing"
  [[ $assume_yes == true ]] && pacman_args+=(--noconfirm)
  sudo pacman "${pacman_args[@]}" "${missing_packages[@]}"
}

offer_omarchy_screensaver_integration() {
  local enable='splinterm integration omarchy-screensaver enable'
  local state

  command -v omarchy-launch-screensaver >/dev/null 2>&1 || return 0
  [[ $(xdg-terminal-exec --print-id 2>/dev/null || true) == *com.oldjobobo.splinterm* ]] || return 0

  state=$(/usr/bin/splinterm integration omarchy-screensaver status 2>/dev/null || true)
  case $state in
  *'integration: enabled'*)
    printf 'Splinterm Omarchy screensaver integration is already enabled.\n'
    return 0
    ;;
  *'incomplete or changed'*)
    printf '%s\n' \
      "Cannot enable Splinterm's Omarchy screensaver integration automatically." \
      '  Review: splinterm integration omarchy-screensaver status' \
      'Splinterm did not modify or replace it.' >&2
    return 0
    ;;
  esac

  if [[ $assume_yes == true || ! -t 0 ]]; then
    printf 'Optional Omarchy screensaver integration was not enabled.\n  Enable: %s\n' "$enable"
    return 0
  fi
  if confirm 'Enable Splinterm for the Omarchy screensaver?'; then
    /usr/bin/splinterm integration omarchy-screensaver enable || printf '%s\n' \
      'Omarchy screensaver integration was not enabled; Splinterm itself is installed.' \
      "  Review: splinterm integration omarchy-screensaver status" >&2
  else
    printf 'Omarchy screensaver integration was not enabled.\n  Enable later: %s\n' "$enable"
  fi
}

source_install() {
  command -v git >/dev/null 2>&1 || {
    printf 'Source installation requires git.\n' >&2
    exit 1
  }
  if [[ -n $(git -C "$root" status --porcelain --untracked-files=no -- . ':(exclude)AGENTS.md') ]]; then
    printf 'The checkout has uncommitted tracked changes. Commit or discard them first.\n' >&2
    exit 1
  fi

  printf '%s\n' \
    'Splinterm source install' \
    '  Build:   current committed checkout' \
    '  Install: locally built Splinterm Arch package' \
    '  Leave:   default terminal and Omarchy configuration unchanged'

  install_dependencies appstream base-devel desktop-file-utils fontconfig \
    freetype2 gcc-libs glibc hicolor-icon-theme libxkbcommon noto-fonts-cjk \
    noto-fonts-emoji pixman pkgconf python rust ttf-jetbrains-mono-nerd \
    wayland xdg-terminal-exec

  local -a build_args=()
  [[ $run_checks == false ]] && build_args+=(--no-check)
  "$root/tools/package/build-local-package.sh" "${build_args[@]}"
  local -a install_args=()
  [[ $assume_yes == true ]] && install_args+=(--yes)
  "$root/tools/package/upgrade-local-package.sh" "${install_args[@]}"
}

release_url() {
  local release=$1 asset=$2
  printf 'https://github.com/%s/releases/download/%s/%s' "$repository" "$release" "$asset"
}

download_release_index() {
  local output=$1
  if [[ ${release_downloader:-} == gh ]]; then
    gh api "repos/$repository/releases?per_page=100" >"$output"
    return
  fi
  curl --fail --location --retry 5 --retry-delay 2 --silent --show-error \
    --header 'Accept: application/vnd.github+json' \
    --header 'User-Agent: splinterm-release-installer' \
    --output "$output" \
    "https://api.github.com/repos/$repository/releases?per_page=100"
}

download_asset() {
  local release=$1 asset=$2 destination=$3
  if [[ ${release_downloader:-} == gh ]]; then
    gh release download "$release" --repo "$repository" --pattern "$asset" \
      --dir "$destination" --clobber
    return
  fi
  curl --fail --location --retry 3 --retry-delay 1 --silent --show-error \
    --output "$destination/$asset" "$(release_url "$release" "$asset")"
}

# Releases before 0.2.0 predate build provenance records. Gate on the release
# version, not on the record's presence, so a package stripped of its record
# cannot skip the check.
release_has_provenance() {
  local major minor
  IFS=. read -r major minor _ <<<"${1%%-*}"
  [[ $major =~ ^[0-9]+$ && $minor =~ ^[0-9]+$ ]] || {
    printf 'The release manifest version is malformed: %s\n' "$1" >&2
    exit 1
  }
  ((10#$major > 0 || 10#$minor >= 2))
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

prebuilt_install() {
  install_dependencies curl desktop-file-utils fontconfig freetype2 gcc-libs glibc \
    hicolor-icon-theme libarchive libxkbcommon noto-fonts-cjk noto-fonts-emoji \
    pixman python ttf-jetbrains-mono-nerd wayland xdg-terminal-exec

  local resolved
  resolved=$(command -v splinterm 2>/dev/null || true)
  if [[ -n $resolved && $resolved != /usr/bin/splinterm ]]; then
    printf 'A shadowing Splinterm client would break trusted UI identity: %s\n' "$resolved" >&2
    printf 'Remove or rename it before installing the Pacman-owned /usr/bin client.\n' >&2
    exit 1
  fi

  if command -v gh >/dev/null 2>&1 && gh auth status --hostname github.com >/dev/null 2>&1; then
    release_downloader=gh
  else
    release_downloader=curl
  fi

  local download_dir stopped=false
  download_dir=$(mktemp -d "${TMPDIR:-/tmp}/splinterm-release.XXXXXX")
  cleanup_prebuilt_install() {
    if [[ $stopped == true ]]; then
      systemctl --user daemon-reload || true
      systemctl --user start splinterd.service || true
    fi
    rm -rf "$download_dir"
  }
  trap cleanup_prebuilt_install EXIT
  if ! download_release_index "$download_dir/releases.json"; then
    printf 'Could not query public releases for %s.\n' "$repository" >&2
    exit 1
  fi

  local -a selected
  mapfile -t selected < <(
    python "$root/tools/package/release-manifest.py" select \
      --repository "$repository" "$download_dir/releases.json"
  )
  if ((${#selected[@]} != 2)); then
    printf 'The release index did not identify one closed versioned release.\n' >&2
    exit 1
  fi
  local release=${selected[0]} manifest_checksum=${selected[1]}
  download_asset "$release" candidate-manifest.json "$download_dir"
  local actual
  actual=$(sha256sum "$download_dir/candidate-manifest.json" | cut -d' ' -f1)
  [[ $actual == "$manifest_checksum" ]] || {
    printf 'Downloaded release manifest checksum does not match the GitHub release record.\n' >&2
    exit 1
  }

  local -a manifest
  mapfile -t manifest < <(
    python "$root/tools/package/release-manifest.py" inspect \
      --repository "$repository" --tag "$release" \
      "$download_dir/candidate-manifest.json"
  )
  if ((${#manifest[@]} != 7)); then
    printf 'The release manifest did not produce the expected package set.\n' >&2
    exit 1
  fi
  local commit=${manifest[0]} version=${manifest[2]}
  local main_asset=${manifest[3]} main_checksum=${manifest[4]}
  local mcp_asset=${manifest[5]} mcp_checksum=${manifest[6]}

  download_asset "$release" "$main_asset" "$download_dir"
  local mcp_installed=false
  pacman -Q splinterm-mcp >/dev/null 2>&1 && mcp_installed=true
  if [[ $mcp_installed == true ]]; then
    download_asset "$release" "$mcp_asset" "$download_dir"
  fi

  actual=$(sha256sum "$download_dir/$main_asset" | cut -d' ' -f1)
  [[ $actual == "$main_checksum" ]] || {
    printf 'Downloaded Splinterm package checksum does not match the release manifest.\n' >&2
    exit 1
  }
  if [[ $mcp_installed == true ]]; then
    actual=$(sha256sum "$download_dir/$mcp_asset" | cut -d' ' -f1)
    [[ $actual == "$mcp_checksum" ]] || {
      printf 'Downloaded MCP package checksum does not match the release manifest.\n' >&2
      exit 1
    }
  fi

  if release_has_provenance "$version"; then
    local provenance_file="$download_dir/package-provenance-v1.json"
    if ! bsdtar -xOf "$download_dir/$main_asset" \
      usr/share/splinterm/package-provenance-v1.json >"$provenance_file"; then
      printf 'Downloaded package omits build provenance.\n' >&2
      exit 1
    fi
    local -a package_provenance
    mapfile -t package_provenance < <(
      python "$root/tools/package/release-manifest.py" inspect-package-provenance \
        --commit "$commit" --version "$version" "$provenance_file"
    )
    if ((${#package_provenance[@]} != 3)); then
      printf 'Downloaded package build provenance does not match the release.\n' >&2
      exit 1
    fi
  else
    printf 'Release %s predates package build provenance; upgrade handoff will not be available.\n' "$version"
  fi

  local candidate mcp_candidate=not-installed
  candidate=$(pacman -Qp "$download_dir/$main_asset" | awk '{print $2}')
  local -a packages=("$download_dir/$main_asset")
  if [[ $mcp_installed == true ]]; then
    mcp_candidate=$(pacman -Qp "$download_dir/$mcp_asset" | awk '{print $2}')
    [[ $candidate == "$mcp_candidate" ]] || {
      printf 'The versioned split-package versions do not match.\n' >&2
      exit 1
    }
    packages+=("$download_dir/$mcp_asset")
  fi

  local was_active=false installed
  systemctl --user is-active --quiet splinterd.service && was_active=true
  installed=$(pacman -Q splinterm 2>/dev/null | awk '{print $2}' || true)
  printf '%s\n' \
    'Splinterm versioned release install' \
    "  Release: $release ($version)" \
    "  Commit:  $commit" \
    "  Package: $candidate" \
    "  MCP:     $mcp_candidate" \
    '  Build:   prebuilt and validated by GitHub Actions' \
    '  Leave:   default terminal and Omarchy configuration unchanged'
  describe_version_change "$installed" "$candidate"
  if [[ $was_active == true ]]; then
    describe_running_daemon
    /usr/bin/splinterm list 2>/dev/null || true
  else
    printf 'splinterd is not running; installing ends nothing.\n'
  fi
  if ! confirm 'Install this prebuilt Splinterm versioned release?'; then
    printf 'Cancelled; nothing was installed, stopped, or restarted.\n'
    exit 0
  fi

  local snapshot
  snapshot="$HOME/.local/state/splinterm/rollback/$(date +%Y%m%d-%H%M%S)-pre-release-${commit:0:12}"
  mkdir -p "$snapshot"
  local binary
  for binary in splinterm splinterd splinterm-mcp; do
    [[ -e /usr/bin/$binary ]] && cp -a "/usr/bin/$binary" "$snapshot/"
  done
  if compgen -G "$snapshot/*" >/dev/null; then
    sha256sum "$snapshot"/* >"$snapshot/sha256sum.txt"
  fi
  pacman -Q splinterm splinterm-mcp >"$snapshot/packages.txt" 2>/dev/null || true

  authenticate_sudo
  if [[ $was_active == true ]]; then
    systemctl --user stop splinterd.service
    stopped=true
  fi
  # The confirmation above was the person's answer; Pacman must not ask again
  # once the daemon is stopped.
  sudo pacman -U --noconfirm "${packages[@]}"
  systemctl --user daemon-reload
  if [[ $was_active == true ]]; then
    systemctl --user start splinterd.service
    stopped=false
  fi

  [[ $(command -v splinterm) == /usr/bin/splinterm ]]
  local -a integrity_packages=(splinterm)
  [[ $mcp_installed == true ]] && integrity_packages+=(splinterm-mcp)
  LC_ALL=C pacman -Qkk "${integrity_packages[@]}" | tee "$download_dir/pacman-qkk.txt"
  if grep -Eq '[1-9][0-9]* altered files|[1-9][0-9]* missing files' "$download_dir/pacman-qkk.txt"; then
    printf 'Pacman reported altered or missing installed files.\n' >&2
    exit 1
  fi
  desktop-file-validate /usr/share/applications/com.oldjobobo.splinterm.desktop
  desktop-file-validate /usr/share/applications/com.oldjobobo.splinterm.remotes.desktop
  if [[ $was_active == true ]]; then
    local daemon_pid daemon_dir
    daemon_pid=$(systemctl --user show splinterd.service -p MainPID --value)
    daemon_dir=$(dirname "$(readlink -f "/proc/$daemon_pid/exe")")
    [[ $(stat -Lc '%d:%i' "$daemon_dir/splinterm") == $(stat -Lc '%d:%i' /usr/bin/splinterm) ]]
    /usr/bin/splinterm list
  fi
  printf 'Splinterm %s installed. Emergency snapshot: %s\n' "$release" "$snapshot"
  rm -rf "$download_dir"
  trap - EXIT
}

if [[ $source_build == true ]]; then
  source_install
else
  prebuilt_install
fi
offer_omarchy_screensaver_integration
