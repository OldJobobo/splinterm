#!/usr/bin/env python3
"""Create a private Fontconfig fixture from makepkg's checksum-verified sources.

Never install fonts or read ambient Fontconfig configuration. Noto fallback
fonts are supplied by the existing Arch runtime dependencies.
"""
import argparse
from pathlib import Path
import shutil
import xml.etree.ElementTree as ET

STYLES = ("Regular", "Bold", "Italic", "BoldItalic")
FALLBACK_DIRS = (
    "/usr/share/fonts/noto-cjk",
    "/usr/share/fonts/noto",
    "/usr/share/fonts/noto-emoji",
)


def prepare(source_dir: Path, output_dir: Path) -> Path:
    sources = [source_dir / f"JetBrainsMonoNerdFont-{style}.ttf" for style in STYLES]
    for source in sources:
        if not source.is_file():
            raise FileNotFoundError(f"missing checked font source: {source}")
    output_dir = output_dir.resolve()
    fonts = output_dir / "fonts"
    cache = output_dir / "cache"
    fonts.mkdir(parents=True, exist_ok=True)
    cache.mkdir(exist_ok=True)
    for source in sources:
        shutil.copyfile(source, fonts / source.name)
    config = ET.Element("fontconfig")
    for directory in (str(fonts), *FALLBACK_DIRS):
        ET.SubElement(config, "dir").text = directory
    ET.SubElement(config, "cachedir").text = str(cache)
    alias = ET.SubElement(config, "alias")
    ET.SubElement(alias, "family").text = "monospace"
    ET.SubElement(ET.SubElement(alias, "prefer"), "family").text = "JetBrains Mono Nerd Font"
    path = output_dir / "fonts.conf"
    ET.ElementTree(config).write(path, encoding="utf-8", xml_declaration=True)
    return path


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("source_dir", type=Path)
    parser.add_argument("output_dir", type=Path)
    args = parser.parse_args()
    print(prepare(args.source_dir, args.output_dir))


if __name__ == "__main__":
    main()
