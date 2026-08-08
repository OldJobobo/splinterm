#!/usr/bin/env bash
set -euo pipefail

root=$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)
repository=OldJobobo/splinterm
channel_branch=edge-channel
assume_yes=false
source_build=false
run_checks=false

usage() {
  cat <<'EOF'
Usage: ./install.sh [--source] [--check] [-y|--yes]

Download and install the newest successful Splinterm edge build on x86_64
Arch/Omarchy without compiling locally.

  --source     Build and install the current committed checkout instead
  --check      Source mode: run the full package test suite (implies --source)
  -y, --yes    Skip dependency and installation confirmations
  -h, --help   Show this help

The default edge installer uses an authenticated GitHub CLI session when
available (required for private repositories), otherwise it uses public release
downloads. It never opts a fresh installation into the optional MCP package; an
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

download_channel_manifest() {
  local output=$1
  if [[ ${release_downloader:-} == gh ]]; then
    gh api "repos/$repository/contents/edge-manifest.json?ref=$channel_branch" \
      --jq .content | tr -d '\n' | base64 --decode >"$output"
    return
  fi
  curl --fail --location --retry 5 --retry-delay 2 --silent --show-error \
    --output "$output" \
    "https://raw.githubusercontent.com/$repository/$channel_branch/edge-manifest.json"
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
  download_dir=$(mktemp -d "${TMPDIR:-/tmp}/splinterm-edge.XXXXXX")
  cleanup_prebuilt_install() {
    if [[ $stopped == true ]]; then
      systemctl --user daemon-reload || true
      systemctl --user start splinterd.service || true
    fi
    rm -rf "$download_dir"
  }
  trap cleanup_prebuilt_install EXIT
  if ! download_channel_manifest "$download_dir/edge-manifest.json"; then
    printf 'Could not download the %s edge release.\n' "$repository" >&2
    printf 'For a private repository, install GitHub CLI and run: gh auth login\n' >&2
    exit 1
  fi

  local -a manifest
  mapfile -t manifest < <(
    python "$root/tools/package/edge-manifest.py" inspect \
      --repository "$repository" "$download_dir/edge-manifest.json"
  )
  if ((${#manifest[@]} != 6)); then
    printf 'The edge manifest did not produce the expected package set.\n' >&2
    exit 1
  fi
  local commit=${manifest[0]} release=${manifest[1]}
  local main_asset=${manifest[2]} main_checksum=${manifest[3]}
  local mcp_asset=${manifest[4]} mcp_checksum=${manifest[5]}

  download_asset "$release" "$main_asset" "$download_dir"
  local mcp_installed=false
  pacman -Q splinterm-mcp >/dev/null 2>&1 && mcp_installed=true
  if [[ $mcp_installed == true ]]; then
    download_asset "$release" "$mcp_asset" "$download_dir"
  fi

  local actual
  actual=$(sha256sum "$download_dir/$main_asset" | cut -d' ' -f1)
  [[ $actual == "$main_checksum" ]] || {
    printf 'Downloaded Splinterm package checksum does not match the edge manifest.\n' >&2
    exit 1
  }
  if [[ $mcp_installed == true ]]; then
    actual=$(sha256sum "$download_dir/$mcp_asset" | cut -d' ' -f1)
    [[ $actual == "$mcp_checksum" ]] || {
      printf 'Downloaded MCP package checksum does not match the edge manifest.\n' >&2
      exit 1
    }
  fi

  local candidate mcp_candidate=not-installed
  candidate=$(pacman -Qp "$download_dir/$main_asset" | awk '{print $2}')
  local -a packages=("$download_dir/$main_asset")
  if [[ $mcp_installed == true ]]; then
    mcp_candidate=$(pacman -Qp "$download_dir/$mcp_asset" | awk '{print $2}')
    [[ $candidate == "$mcp_candidate" ]] || {
      printf 'The edge split-package versions do not match.\n' >&2
      exit 1
    }
    packages+=("$download_dir/$mcp_asset")
  fi

  local was_active=false
  systemctl --user is-active --quiet splinterd.service && was_active=true
  printf '%s\n' \
    'Splinterm edge install' \
    "  Commit:  $commit" \
    "  Package: $candidate" \
    "  MCP:     $mcp_candidate" \
    '  Build:   prebuilt and validated by GitHub Actions' \
    '  Leave:   default terminal and Omarchy configuration unchanged'
  if [[ $was_active == true ]]; then
    printf 'splinterd is running; installation will end its daemon-owned shells.\n'
    /usr/bin/splinterm list 2>/dev/null || true
  fi
  confirm 'Install this prebuilt Splinterm edge package?' || exit 0

  local snapshot
  snapshot="$HOME/.local/state/splinterm/rollback/$(date +%Y%m%d-%H%M%S)-pre-edge-${commit:0:12}"
  mkdir -p "$snapshot"
  local binary
  for binary in splinterm splinterd splinterm-mcp; do
    [[ -e /usr/bin/$binary ]] && cp -a "/usr/bin/$binary" "$snapshot/"
  done
  if compgen -G "$snapshot/*" >/dev/null; then
    sha256sum "$snapshot"/* >"$snapshot/sha256sum.txt"
  fi
  pacman -Q splinterm splinterm-mcp >"$snapshot/packages.txt" 2>/dev/null || true

  if [[ $was_active == true ]]; then
    systemctl --user stop splinterd.service
    stopped=true
  fi
  local -a pacman_args=(-U)
  [[ $assume_yes == true ]] && pacman_args+=(--noconfirm)
  sudo pacman "${pacman_args[@]}" "${packages[@]}"
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
  if [[ $was_active == true ]]; then
    local daemon_pid daemon_dir
    daemon_pid=$(systemctl --user show splinterd.service -p MainPID --value)
    daemon_dir=$(dirname "$(readlink -f "/proc/$daemon_pid/exe")")
    [[ $(stat -Lc '%d:%i' "$daemon_dir/splinterm") == $(stat -Lc '%d:%i' /usr/bin/splinterm) ]]
    /usr/bin/splinterm list
  fi
  printf 'Splinterm edge %s installed. Emergency snapshot: %s\n' "${commit:0:12}" "$snapshot"
  rm -rf "$download_dir"
  trap - EXIT
}

if [[ $source_build == true ]]; then
  source_install
else
  prebuilt_install
fi
