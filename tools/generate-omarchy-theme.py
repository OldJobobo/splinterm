#!/usr/bin/env python3
"""Generate Splinterm's bounded theme JSON from an Omarchy colors.toml file."""

from __future__ import annotations

import argparse
import json
import os
from pathlib import Path
import tempfile
import tomllib

ROLE_ALIASES = {
    "accent": ("accent", "cursor"),
    "bg": ("bg", "background"),
    "darker_bg": ("darker_bg", "color0"),
    "selection": ("selection", "selection_background"),
    "muted": ("muted", "color8"),
    "fg": ("fg", "foreground"),
    "bright_fg": ("bright_fg", "color15"),
    "red": ("red", "color1"),
    "green": ("green", "color2"),
    "yellow": ("yellow", "color3"),
    "blue": ("blue", "color4"),
    "magenta": ("magenta", "color5"),
    "cyan": ("cyan", "color6"),
    "bright_red": ("bright_red", "color9"),
    "bright_green": ("bright_green", "color10"),
    "bright_yellow": ("bright_yellow", "color11"),
    "bright_blue": ("bright_blue", "color12"),
    "bright_magenta": ("bright_magenta", "color13"),
    "bright_cyan": ("bright_cyan", "color14"),
}


def parse_foot_bool(value: str) -> bool:
    normalized = value.strip().lower()
    if normalized in {"yes", "true", "on", "1"}:
        return True
    if normalized in {"no", "false", "off", "0"}:
        return False
    raise ValueError("foot.ini blur must be a boolean")


def theme_settings(theme_dir: Path) -> tuple[float, bool]:
    foot = theme_dir / "foot.ini"
    if not foot.is_file():
        return 1.0, False

    assignments: dict[str, dict[str, str]] = {
        "colors": {},
        "colors-dark": {},
    }
    sections_seen: set[str] = set()
    section = ""
    for raw_line in foot.read_text(encoding="utf-8").splitlines():
        line = raw_line.strip()
        if line.startswith("[") and line.endswith("]"):
            section = line[1:-1].strip().lower()
            if section in assignments:
                sections_seen.add(section)
            continue
        if section not in assignments:
            continue
        key, separator, value = line.partition("=")
        normalized_key = key.strip().lower()
        if separator and normalized_key in {"alpha", "blur"}:
            assignments[section][normalized_key] = value.strip()

    selected = "colors-dark" if "colors-dark" in sections_seen else "colors"
    values = assignments[selected]
    try:
        alpha = float(values.get("alpha", "1.0"))
    except ValueError as error:
        raise ValueError("foot.ini alpha must be a number") from error
    if not 0.0 <= alpha <= 1.0:
        raise ValueError("foot.ini alpha must be between 0.0 and 1.0")
    blur = parse_foot_bool(values["blur"]) if "blur" in values else False
    return alpha, blur


def generate(
    colors: dict[str, object], alpha: float = 1.0, blur: bool = False
) -> dict[str, object]:
    roles = {
        role: next((colors[key] for key in aliases if key in colors), None)
        for role, aliases in ROLE_ALIASES.items()
    }
    roles["selection_foreground"] = colors.get(
        "selection_foreground", roles["fg"]
    )
    roles["active_tab_background"] = colors.get(
        "active_tab_background", roles["selection"]
    )
    missing = [name for name, value in roles.items() if value is None]
    if missing:
        raise ValueError("missing Omarchy roles: " + ", ".join(missing))
    for name, value in roles.items():
        if not isinstance(value, str) or len(value) != 7 or not value.startswith("#"):
            raise ValueError(f"{name} must be #RRGGBB")
        int(value[1:], 16)
    if not 0.0 <= alpha <= 1.0:
        raise ValueError("alpha must be between 0.0 and 1.0")
    if not isinstance(blur, bool):
        raise ValueError("blur must be a boolean")
    return {
        "background": roles["bg"],
        "alpha": float(alpha),
        "blur": blur,
        "foreground": roles["fg"],
        "cursor": roles["accent"],
        "selection": roles["selection"],
        "selection_foreground": roles["selection_foreground"],
        "active_tab_background": roles["active_tab_background"],
        "url": roles["blue"],
        "ui_accent": roles["accent"],
        "pane_border": roles["muted"],
        "pane_border_active": roles["accent"],
        "ansi": [
            roles["darker_bg"], roles["red"], roles["green"], roles["yellow"],
            roles["blue"], roles["magenta"], roles["cyan"], roles["fg"],
            roles["muted"], roles["bright_red"], roles["bright_green"],
            roles["bright_yellow"], roles["bright_blue"], roles["bright_magenta"],
            roles["bright_cyan"], roles["bright_fg"],
        ],
    }


def main() -> int:
    parser = argparse.ArgumentParser()
    parser.add_argument("source", type=Path, help="Omarchy theme colors.toml")
    parser.add_argument(
        "--output", type=Path,
        default=Path(os.environ.get("XDG_CONFIG_HOME", Path.home() / ".config")) / "splinterm/theme.json",
    )
    args = parser.parse_args()
    with args.source.open("rb") as source:
        alpha, blur = theme_settings(args.source.parent)
        generated = generate(tomllib.load(source), alpha, blur)
    args.output.parent.mkdir(parents=True, exist_ok=True, mode=0o700)
    fd, temporary = tempfile.mkstemp(prefix=".theme.", dir=args.output.parent, text=True)
    try:
        with os.fdopen(fd, "w", encoding="utf-8") as target:
            json.dump(generated, target, indent=2)
            target.write("\n")
        os.replace(temporary, args.output)
    finally:
        if os.path.exists(temporary):
            os.unlink(temporary)
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
