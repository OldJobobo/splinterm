#!/usr/bin/env python3
"""Validate the private Arch package without installing it."""

from __future__ import annotations

import argparse
import json
import os
from pathlib import Path
import shutil
import stat
import subprocess
import tempfile

REQUIRED = {
    "usr/bin/generate-omarchy-theme.py",
    "usr/bin/splinterd",
    "usr/bin/splinterm",
    "usr/bin/splinterm-pty-child",
    "usr/bin/splinterm-xdg-terminal-exec",
    "usr/lib/systemd/user/splinterd.service",
    "usr/share/applications/com.oldjobobo.splinterm.desktop",
    "usr/share/icons/hicolor/scalable/apps/com.oldjobobo.splinterm.svg",
    "usr/share/metainfo/com.oldjobobo.splinterm.metainfo.xml",
    "usr/share/doc/splinterm/config.ini",
    "usr/share/doc/splinterm/omarchy/10-splinterm.sh",
    "usr/share/doc/splinterm/packaging.md",
    "usr/share/doc/splinterm/theme.json",
    "usr/share/doc/splinterm/xdg-terminals.list",
    "usr/share/licenses/splinterm/LICENSE",
    "usr/share/licenses/splinterm/THIRD_PARTY.md",
}
EXECUTABLES = {
    "usr/bin/generate-omarchy-theme.py",
    "usr/bin/splinterd",
    "usr/bin/splinterm",
    "usr/bin/splinterm-pty-child",
    "usr/bin/splinterm-xdg-terminal-exec",
}
RUNTIME_DEPENDENCIES = {
    "fontconfig",
    "freetype2",
    "gcc-libs",
    "glibc",
    "libxkbcommon",
    "noto-fonts-cjk",
    "noto-fonts-emoji",
    "pixman",
    "ttf-jetbrains-mono-nerd-basic",
    "wayland",
    "xdg-terminal-exec",
}


def run(command: list[str], **kwargs) -> subprocess.CompletedProcess[str]:
    kwargs.setdefault("check", True)
    kwargs.setdefault("text", True)
    return subprocess.run(command, **kwargs)


def package_entries(package: Path) -> set[str]:
    listing = run(["bsdtar", "-tf", str(package)], capture_output=True).stdout
    return {line.removeprefix("./").rstrip("/") for line in listing.splitlines() if line}


def validate_launcher(root: Path) -> None:
    with tempfile.TemporaryDirectory(prefix="splinterm-launcher-") as directory:
        fake = Path(directory)
        state = fake / "state"
        record = fake / "record"
        (fake / "splinterm").write_text(
            "#!/bin/sh\n"
            "if [ \"$1\" = ping ]; then\n"
            "  count=0; [ ! -f \"$SPLINTERM_TEST_STATE/count\" ] || count=$(cat \"$SPLINTERM_TEST_STATE/count\")\n"
            "  count=$((count + 1)); printf '%s' \"$count\" >\"$SPLINTERM_TEST_STATE/count\"\n"
            "  [ -f \"$SPLINTERM_TEST_STATE/ready\" ]; exit\n"
            "fi\n"
            "printf '%s\\n' \"$@\" >\"$SPLINTERM_TEST_RECORD\"\n",
            encoding="utf-8",
        )
        (fake / "systemctl").write_text(
            "#!/bin/sh\n"
            "printf '%s\\n' \"$*\" >>\"$SPLINTERM_TEST_STATE/systemctl\"\n"
            "[ \"$2\" != restart ] || : >\"$SPLINTERM_TEST_STATE/ready\"\n",
            encoding="utf-8",
        )
        for executable in (fake / "splinterm", fake / "systemctl"):
            executable.chmod(0o755)
        state.mkdir()
        environment = os.environ.copy()
        environment.update(
            PATH=f"{fake}:{root / 'usr/bin'}:{environment['PATH']}",
            SPLINTERM_TEST_STATE=str(state),
            SPLINTERM_TEST_RECORD=str(record),
        )
        run(
            [str(root / "usr/bin/splinterm-xdg-terminal-exec"), "--working-directory", "/tmp/a b"],
            env=environment,
            timeout=10,
        )
        calls = (state / "systemctl").read_text(encoding="utf-8").splitlines()
        assert calls == ["--user start splinterd.service", "--user restart splinterd.service"]
        assert record.read_text(encoding="utf-8").splitlines() == [
            "launch",
            "--working-directory",
            "/tmp/a b",
        ]


def validate_theme_generator(root: Path) -> None:
    colors = {
        "accent": "#010203", "bg": "#101112", "darker_bg": "#000000",
        "selection": "#202122", "muted": "#303132", "fg": "#d0d1d2",
        "bright_fg": "#ffffff", "red": "#800000", "yellow": "#808000",
        "green": "#008000", "cyan": "#008080", "blue": "#000080",
        "magenta": "#800080", "bright_red": "#ff0000",
        "bright_yellow": "#ffff00", "bright_green": "#00ff00",
        "bright_cyan": "#00ffff", "bright_blue": "#0000ff",
        "bright_magenta": "#ff00ff",
    }
    with tempfile.TemporaryDirectory(prefix="splinterm-theme-") as directory:
        directory = Path(directory)
        source = directory / "colors.toml"
        source.write_text("".join(f'{key} = "{value}"\n' for key, value in colors.items()))
        output = directory / "theme.json"
        run([str(root / "usr/bin/generate-omarchy-theme.py"), str(source), "--output", str(output)])
        generated = json.loads(output.read_text(encoding="utf-8"))
        assert generated["background"] == colors["bg"]
        assert generated["cursor"] == colors["accent"]
        assert len(generated["ansi"]) == 16


def main() -> int:
    parser = argparse.ArgumentParser()
    parser.add_argument("package", type=Path)
    args = parser.parse_args()
    package = args.package.resolve()
    if not package.is_file() or package.stat().st_size > 256 * 1024 * 1024:
        raise SystemExit("package is missing or unexpectedly large")

    entries = package_entries(package)
    missing = REQUIRED - entries
    assert not missing, f"missing package paths: {sorted(missing)}"
    forbidden = [
        entry for entry in entries
        if entry.startswith(("home/", "etc/", "usr/share/omarchy/"))
    ]
    assert not forbidden, f"package mutates forbidden paths: {forbidden}"

    with tempfile.TemporaryDirectory(prefix="splinterm-package-root-") as directory:
        root = Path(directory)
        run(["bsdtar", "-xf", str(package), "-C", str(root)])
        for relative in EXECUTABLES:
            mode = (root / relative).stat().st_mode
            assert mode & stat.S_IXUSR, f"{relative} is not executable"
        for binary in ("splinterm", "splinterd", "splinterm-pty-child"):
            result = run(["ldd", str(root / "usr/bin" / binary)], capture_output=True)
            assert "not found" not in result.stdout

        desktop = root / "usr/share/applications/com.oldjobobo.splinterm.desktop"
        metadata = root / "usr/share/metainfo/com.oldjobobo.splinterm.metainfo.xml"
        run(["desktop-file-validate", str(desktop)])
        run(["appstreamcli", "validate", "--no-net", str(metadata)])
        assert "Exec=splinterm-xdg-terminal-exec" in desktop.read_text(encoding="utf-8")
        assert "Icon=com.oldjobobo.splinterm" in desktop.read_text(encoding="utf-8")

        pkginfo = (root / ".PKGINFO").read_text(encoding="utf-8")
        dependencies = {
            line.split(" = ", 1)[1].split(">", 1)[0].split("<", 1)[0].split("=", 1)[0]
            for line in pkginfo.splitlines() if line.startswith("depend = ")
        }
        assert RUNTIME_DEPENDENCIES <= dependencies
        validate_theme_generator(root)
        validate_launcher(root)

    print(f"Package validation passed: {package}")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
