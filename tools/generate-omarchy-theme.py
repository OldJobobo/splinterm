#!/usr/bin/env python3
"""Generate Splinterm's bounded theme JSON from an Omarchy colors.toml file."""

from __future__ import annotations

import argparse
import json
import os
from pathlib import Path
import tempfile
import tomllib

REQUIRED = (
    "accent", "bg", "darker_bg", "selection", "muted", "fg", "bright_fg",
    "red", "yellow", "green", "cyan", "blue", "magenta",
    "bright_red", "bright_yellow", "bright_green", "bright_cyan",
    "bright_blue", "bright_magenta",
)


def generate(colors: dict[str, object]) -> dict[str, object]:
    missing = [name for name in REQUIRED if name not in colors]
    if missing:
        raise ValueError("missing Omarchy roles: " + ", ".join(missing))
    for name in REQUIRED:
        value = colors[name]
        if not isinstance(value, str) or len(value) != 7 or not value.startswith("#"):
            raise ValueError(f"{name} must be #RRGGBB")
        int(value[1:], 16)
    return {
        "background": colors["bg"],
        "foreground": colors["fg"],
        "cursor": colors["accent"],
        "selection": colors["selection"],
        "url": colors["blue"],
        "ui_accent": colors["accent"],
        "pane_border": colors["muted"],
        "pane_border_active": colors["accent"],
        "ansi": [
            colors["darker_bg"], colors["red"], colors["green"], colors["yellow"],
            colors["blue"], colors["magenta"], colors["cyan"], colors["fg"],
            colors["muted"], colors["bright_red"], colors["bright_green"],
            colors["bright_yellow"], colors["bright_blue"], colors["bright_magenta"],
            colors["bright_cyan"], colors["bright_fg"],
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
        generated = generate(tomllib.load(source))
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
