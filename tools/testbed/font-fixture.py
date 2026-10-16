#!/usr/bin/env python3
"""Build private Fontconfig fixtures for live font-change acceptance.

`invalid-candidate` makes Fontconfig resolve `monospace` to a primary regular
face that Splinterm must reject rather than skip:

1. copy an installed font into a private directory;
2. rename its family at scan time to a family only the fixture provides;
3. index it into a private cache directory with a build-only configuration,
   so no shared cache is written;
4. truncate the copy in place and zero what remains, without changing the
   directory, so the cache keeps resolving it.

Splinterm's `fc-match` then returns the fixture path, the file is a non-empty
regular file (the resolver accepts it), and parsing the face fails because it
no longer starts with an sfnt tag. That rejects the whole candidate generation
and keeps the last valid font in every build profile.

The generated `fonts.conf` is a user-level file: install it as
`$XDG_CONFIG_HOME/fontconfig/fonts.conf` for the process under test. It never
installs fonts, touches shared caches, or edits the ambient configuration.
"""

from __future__ import annotations

import argparse
import json
import os
from pathlib import Path
import shutil
import subprocess
import sys
from typing import NoReturn
from xml.etree import ElementTree as ET


DEFAULT_FAMILY = "Splinterm Invalid Candidate"
FIXTURE_STEM = "splinterm-invalid-candidate"
FONT_SUFFIXES = (".ttf", ".otf", ".ttc", ".otc")
# Non-empty, so the resolver accepts the file, and all zero, so it has no sfnt
# tag and face parsing fails. A truncation that keeps the original tag parses
# lazily and only fails later, in glyph metrics, where debug builds panic
# inside the parser instead of rejecting.
TRUNCATED_BYTES = 8
SYSTEM_CONFIG = Path("/etc/fonts/fonts.conf")
CACHE_SUFFIX_MARK = ".cache-"


def fail(message: str) -> NoReturn:
    raise RuntimeError(message)


def fontconfig_environment(config: Path | None) -> dict[str, str]:
    environment = os.environ.copy()
    # Fontconfig stamps and validates caches with SOURCE_DATE_EPOCH when it is
    # set (as reproducible-build shells do), which would discard the fixture
    # cache as stale. The fixture must use real timestamps.
    environment.pop("SOURCE_DATE_EPOCH", None)
    if config is None:
        environment.pop("FONTCONFIG_FILE", None)
    else:
        environment["FONTCONFIG_FILE"] = str(config)
    return environment


def fc_match(config: Path | None, pattern: str, sorted_: bool = False) -> list[str]:
    environment = fontconfig_environment(config)
    arguments = ["fc-match", "-f", "%{file}\\n", pattern]
    if sorted_:
        arguments.insert(1, "-s")
    result = subprocess.run(
        arguments, env=environment, text=True, capture_output=True, check=True
    )
    return [line for line in result.stdout.splitlines() if line]


def write_config(path: Path, elements: list[ET.Element]) -> None:
    root = ET.Element("fontconfig")
    root.extend(elements)
    ET.indent(root)
    path.write_text(
        '<?xml version="1.0"?>\n<!DOCTYPE fontconfig SYSTEM "urn:fontconfig:fonts.dtd">\n'
        + ET.tostring(root, encoding="unicode")
        + "\n",
        encoding="utf-8",
    )
    path.chmod(0o600)


def text_element(tag: str, text: str, **attributes: str) -> ET.Element:
    element = ET.Element(tag, attributes)
    element.text = text
    return element


def scan_rename(font: Path, family: str) -> ET.Element:
    match = ET.Element("match", {"target": "scan"})
    test = ET.SubElement(match, "test", {"name": "file"})
    test.append(text_element("string", str(font)))
    edit = ET.SubElement(
        match, "edit", {"name": "family", "mode": "assign_replace", "binding": "same"}
    )
    edit.append(text_element("string", family))
    return match


def strong_monospace(family: str) -> ET.Element:
    # The shape Omarchy's font selection uses: a strong family prepend on the
    # generic monospace request, which outranks distribution weak aliases.
    match = ET.Element("match", {"target": "pattern"})
    test = ET.SubElement(match, "test", {"qual": "any", "name": "family"})
    test.append(text_element("string", "monospace"))
    edit = ET.SubElement(
        match, "edit", {"name": "family", "mode": "prepend", "binding": "strong"}
    )
    edit.append(text_element("string", family))
    return match


def invalid_candidate(output: Path, source: Path | None, family: str) -> dict[str, object]:
    output = output.absolute()
    if output.exists() or output.is_symlink():
        fail(f"fixture output already exists: {output}")
    if not output.parent.is_dir():
        fail(f"fixture parent directory is missing: {output.parent}")
    if source is None:
        matches = fc_match(None, "monospace:style=Regular")
        if not matches:
            fail("ambient Fontconfig resolved no monospace font to copy")
        source = Path(matches[0])
    source = source.resolve()
    if not source.is_file() or source.suffix.lower() not in FONT_SUFFIXES:
        fail(f"fixture source is not a font file: {source}")

    output.mkdir(mode=0o700)
    fonts = output / "fonts"
    cache = output / "cache"
    fonts.mkdir(mode=0o700)
    cache.mkdir(mode=0o700)
    font = fonts / f"{FIXTURE_STEM}{source.suffix.lower()}"
    shutil.copyfile(source, font)
    font.chmod(0o600)

    build = output / "build.conf"
    write_config(
        build,
        [
            text_element("dir", str(fonts)),
            text_element("cachedir", str(cache)),
            scan_rename(font, family),
        ],
    )
    subprocess.run(
        ["fc-cache", "-f", str(fonts)],
        env=fontconfig_environment(build),
        text=True,
        capture_output=True,
        check=True,
    )
    if not any(CACHE_SUFFIX_MARK in path.name for path in cache.iterdir()):
        fail(f"fc-cache wrote no private cache under {cache}")

    user_config = output / "fonts.conf"
    write_config(
        user_config,
        [
            text_element("dir", str(fonts)),
            text_element("cachedir", str(cache)),
            scan_rename(font, family),
            strong_monospace(family),
        ],
    )
    # Model the installed layout: the system configuration followed by this
    # user-level file, as the system `50-user` include does.
    verify = output / "verify.conf"
    includes = []
    if SYSTEM_CONFIG.is_file():
        includes.append(text_element("include", str(SYSTEM_CONFIG), ignore_missing="no"))
    includes.append(text_element("include", str(user_config), ignore_missing="no"))
    write_config(verify, includes)

    if fc_match(verify, "monospace:style=Regular")[:1] != [str(font)]:
        fail("the fixture does not win the monospace alias before truncation")

    directory_mtime = fonts.stat().st_mtime_ns
    with font.open("r+b") as handle:
        handle.truncate(TRUNCATED_BYTES)
        handle.seek(0)
        handle.write(bytes(TRUNCATED_BYTES))
    if fonts.stat().st_mtime_ns != directory_mtime:
        fail("truncation changed the fixture directory and invalidated its cache")

    resolved = fc_match(verify, "monospace:style=Regular")
    if resolved[:1] != [str(font)]:
        fail(f"the cached fixture no longer resolves after truncation: {resolved[:1]}")
    if font.stat().st_size != TRUNCATED_BYTES:
        fail("the fixture was not truncated")
    if font.read_bytes() != bytes(TRUNCATED_BYTES):
        fail("the fixture still holds font bytes")
    scanned = subprocess.run(
        ["fc-scan", "--format", "%{file}\\n", str(font)],
        env=fontconfig_environment(build),
        text=True,
        capture_output=True,
        check=False,
    )
    if scanned.stdout.strip():
        fail("the truncated fixture still scans as a font")

    return {
        "family": family,
        "font": str(font),
        "font_bytes": TRUNCATED_BYTES,
        "source": str(source),
        "user_config": str(user_config),
        "verify_config": str(verify),
        "monospace_first_match": resolved[0],
    }


def parser() -> argparse.ArgumentParser:
    result = argparse.ArgumentParser(description=__doc__.split("\n", 1)[0])
    subcommands = result.add_subparsers(dest="fixture", required=True)
    invalid = subcommands.add_parser(
        "invalid-candidate",
        help="monospace resolves to a cached but truncated regular face",
    )
    invalid.add_argument("--output", required=True, type=Path)
    invalid.add_argument("--source", type=Path)
    invalid.add_argument("--family", default=DEFAULT_FAMILY)
    return result


def main() -> int:
    arguments = parser().parse_args()
    try:
        summary = invalid_candidate(arguments.output, arguments.source, arguments.family)
    except (RuntimeError, OSError, subprocess.CalledProcessError) as error:
        if isinstance(error, subprocess.CalledProcessError) and error.stderr:
            print(error.stderr.rstrip(), file=sys.stderr)
        print(f"font-fixture: {error}", file=sys.stderr)
        return 1
    print(json.dumps(summary, indent=2, sort_keys=True))
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
