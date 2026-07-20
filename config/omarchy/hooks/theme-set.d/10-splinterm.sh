#!/usr/bin/env bash
# Omarchy 4 native theme hook. Install beside other theme-set.d integrations.
set -euo pipefail

config_home=${XDG_CONFIG_HOME:-"$HOME/.config"}
state_home=${XDG_STATE_HOME:-"$HOME/.local/state"}
generator=${SPLINTERM_THEME_GENERATOR:-generate-omarchy-theme.py}

if ! command -v "$generator" >/dev/null 2>&1; then
  printf 'Skipped: Splinterm theme generator is not installed\n'
  exit 0
fi

for source in \
  "$state_home/omarchy/current/theme/colors.toml" \
  "$config_home/omarchy/current/theme/colors.toml"
do
  if [[ -f "$source" ]]; then
    "$generator" "$source" --output "$config_home/splinterm/theme.json"
    printf 'Splinterm theme updated!\n'
    exit 0
  fi
done

printf 'Skipped: active Omarchy colors.toml is unavailable\n'
