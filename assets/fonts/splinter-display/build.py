#!/usr/bin/env python3
"""Build original Splinter Display fonts. No source font is imported."""

import argparse
import json
from pathlib import Path

from fontTools.feaLib.builder import addOpenTypeFeaturesFromString
from fontTools.fontBuilder import FontBuilder
from fontTools.pens.areaPen import AreaPen
from fontTools.pens.boundsPen import BoundsPen
from fontTools.pens.cu2quPen import Cu2QuPen
from fontTools.pens.recordingPen import RecordingPen
from fontTools.pens.reverseContourPen import ReverseContourPen
from fontTools.pens.t2CharStringPen import T2CharStringPen
from fontTools.pens.ttGlyphPen import TTGlyphPen
from fontTools.svgLib.path import parse_path

from alphabet import companion_glyphs
from kerning import KERN

ROOT = Path(__file__).resolve().parent
EPOCH = 3860000000  # Fixed OpenType timestamp; never depend on build time.
FAMILY = "Splinter Display"
STEM = "SplinterDisplay-Heavy"
RELEASE = "0.1"
VERSION = "0.100"  # OpenType's three-decimal spelling of the v0.1 asset release.


def load_glyphs():
    seed = json.loads((ROOT / "glyphs.json").read_text(encoding="utf-8"))
    return {".notdef": {"advance": 600, "contours": [
        "M 45 0 L 45 700 L 555 700 L 555 0 Z",
        "M 115 70 L 485 70 L 485 630 L 115 630 Z",
    ], "holes": [1]}, **companion_glyphs(seed), **seed}


def outline(spec):
    """Normalize outer contours clockwise and counters counterclockwise."""
    result = RecordingPen()
    for index, path in enumerate(spec["contours"]):
        area = AreaPen()
        parse_path(path, area)
        clockwise = index not in spec.get("holes", [])
        target = result if (area.value < 0) == clockwise else ReverseContourPen(result)
        parse_path(path, target)
    return result


def build_font(glyphs, destination, *, ttf):
    builder = FontBuilder(1000, isTTF=ttf)
    builder.setupGlyphOrder(list(glyphs))
    cmap = {g["codepoint"]: name for name, g in glyphs.items() if "codepoint" in g}
    builder.setupCharacterMap(cmap)
    metrics, outlines = {}, {}
    for name, spec in glyphs.items():
        drawing = outline(spec)
        bounds = BoundsPen(None)
        drawing.replay(bounds)
        metrics[name] = (spec["advance"], round(bounds.bounds[0]) if bounds.bounds else 0)
        if ttf:
            pen = TTGlyphPen(None)
            drawing.replay(Cu2QuPen(pen, max_err=0.5, all_quadratic=True))
            outlines[name] = pen.glyph()
        else:
            pen = T2CharStringPen(spec["advance"], None)
            drawing.replay(pen)
            outlines[name] = pen.getCharString()
    if ttf:
        builder.setupGlyf(outlines)
    else:
        builder.setupCFF(STEM, {
            "FullName": f"{FAMILY} Heavy", "FamilyName": FAMILY,
            "Weight": "Heavy", "version": VERSION,
        }, outlines, {})
    builder.setupHorizontalMetrics(metrics)
    builder.setupHorizontalHeader(ascent=820, descent=-240, lineGap=0)
    # Legacy RIBBI family includes the weight; typographic names group it as Heavy.
    builder.setupNameTable({
        "familyName": f"{FAMILY} Heavy", "styleName": "Regular",
        "typographicFamily": FAMILY, "typographicSubfamily": "Heavy",
        "uniqueFontIdentifier": f"{STEM}-{VERSION}",
        "fullName": f"{FAMILY} Heavy", "psName": STEM,
        "version": f"Version {VERSION}",
        "copyright": "Copyright (c) 2025 Splinterm contributors",
        "licenseDescription": (ROOT.parents[2] / "LICENSE").read_text(encoding="utf-8"),
        "description": "Original heavy display face. Printable ASCII and common typographic punctuation. Proportional letters, tabular lining figures.",
    })
    builder.setupOS2(version=4, sTypoAscender=820, sTypoDescender=-240, sTypoLineGap=0,
                     usWinAscent=820, usWinDescent=240, sxHeight=520, sCapHeight=720,
                     usWeightClass=900, usWidthClass=5, fsSelection=0xC0, fsType=0)
    builder.setupPost(underlinePosition=-145, underlineThickness=90, isFixedPitch=0)
    builder.setupMaxp()
    builder.font["head"].created = EPOCH
    builder.font["head"].modified = EPOCH
    builder.font["head"].fontRevision = float(VERSION)
    builder.font["head"].macStyle = 0
    pairs = "\n".join(f"pos {cmap[ord(left)]} {cmap[ord(right)]} {value};"
                      for (left, right), value in sorted(KERN.items()))
    addOpenTypeFeaturesFromString(builder.font,
                                 "languagesystem DFLT dflt;\n"
                                 "languagesystem latn dflt;\n"
                                 f"feature kern {{\n{pairs}\n}} kern;")
    builder.font.recalcTimestamp = False
    builder.save(destination)
    if ttf:
        builder.font.flavor = "woff2"
        builder.save(destination.with_suffix(".woff2"))


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--output", type=Path, default=ROOT / "dist")
    args = parser.parse_args()
    args.output.mkdir(parents=True, exist_ok=True)
    glyphs = load_glyphs()
    for extension in ("otf", "ttf"):
        build_font(glyphs, args.output / f"{STEM}.{extension}", ttf=extension == "ttf")
    # Imported here to keep the compiler usable without Pillow.
    from specimen import svg_specimen
    svg_specimen(glyphs, args.output / "specimen.svg")
    print(f"Built {len(glyphs) - 1} mapped characters + .notdef in {args.output}")


if __name__ == "__main__":
    main()
