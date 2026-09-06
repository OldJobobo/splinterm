#!/usr/bin/env python3
"""Generate outline and actual-font proof sheets without opening a desktop Window."""

import argparse
from html import escape
from pathlib import Path

from fontTools.pens.svgPathPen import SVGPathPen

import build
from kerning import KERN

WIDTH, HEIGHT = 1400, 2200
INK, MUTED, GROUND, SIGNAL = "#f2e6d8", "#b2a697", "#201e1f", "#f3a536"
# (text, x, baseline, em size); shared by the SVG and font-rendered proof.
ROWS = [
    ("splinterm", 64, 340, 286),
    ("ABCDEFGHIJKLM", 64, 630, 118),
    ("NOPQRSTUVWXYZ", 64, 762, 118),
    ("abcdefghijklm", 64, 959, 139),
    ("nopqrstuvwxyz", 64, 1120, 139),
    ("0123456789", 64, 1330, 174),
    ("!\"#$%&'()*+,-./:;<=>?", 64, 1510, 86),
    ("@[\\]^_`{|}~ ‘quotes’ “type” – — … •", 64, 1628, 66),
    ("Build boldly. Split freely.", 64, 1830, 104),
    ("AVATAR / WAYFINDER / 09:41", 64, 1938, 70),
    ("The quick brown fox jumps over the lazy dog.", 64, 2042, 55),
]
LABELS = [
    ("SPLINTER DISPLAY / ORIGINAL TYPEFACE", 64, 60, 18),
    (f"HEAVY / v{build.RELEASE}", 1150, 60, 15),
    ("The approved letterforms. Now a full display alphabet.", 64, 450, 20),
    ("UPPERCASE", 64, 510, 15),
    ("LOWERCASE", 64, 843, 15),
    ("TABULAR LINING FIGURES", 64, 1188, 15),
    ("PUNCTUATION", 64, 1400, 15),
    ("IN USE", 64, 1718, 15),
    ("104 characters / proportional letters / one heavy weight / no hinting", 64, 2135, 17),
]


def character_map(glyphs):
    return {chr(g["codepoint"]): g for g in glyphs.values() if "codepoint" in g}


def advance(text, cmap):
    return sum(cmap[c]["advance"] for c in text) + sum(KERN.get(pair, 0) for pair in zip(text, text[1:]))


def word_paths(text, cmap, x, baseline, size, *, accent=False):
    scale = size / 1000
    cursor, last, parts = 0, None, []
    for index, character in enumerate(text):
        cursor += KERN.get((last, character), 0)
        pen = SVGPathPen(None)
        build.outline(cmap[character]).replay(pen)
        color = SIGNAL if accent and index == len(text) - 1 else INK
        parts.append(f'<path fill="{color}" d="{pen.getCommands()}" '
                     f'transform="translate({x + cursor * scale:g} {baseline:g}) '
                     f'scale({scale:g} {-scale:g})"/>')
        cursor += cmap[character]["advance"]
        last = character
    return "\n".join(parts)


def svg_specimen(glyphs, destination):
    cmap = character_map(glyphs)
    parts = [f'<svg xmlns="http://www.w3.org/2000/svg" width="{WIDTH}" height="{HEIGHT}" '
             f'viewBox="0 0 {WIDTH} {HEIGHT}" role="img" aria-labelledby="title desc">',
             '<title id="title">Splinter Display Heavy — full alphabet</title>',
             '<desc id="desc">104 mapped characters: printable ASCII, nonbreaking space, '
             'curly quotes, en and em dashes, ellipsis, and bullet.</desc>',
             f'<rect width="{WIDTH}" height="{HEIGHT}" fill="{GROUND}"/>']
    for text, x, baseline, size in ROWS:
        assert x + advance(text, cmap) * size / 1000 <= WIDTH - 40, text
        parts.append(word_paths(text, cmap, x, baseline, size, accent=text == "splinterm"))
    for text, x, y, size in LABELS:
        parts.append(f'<text x="{x}" y="{y}" fill="{MUTED}" font-size="{size}" '
                     f'font-family="DejaVu Sans, sans-serif">{escape(text)}</text>')
    parts.append(f'<text x="64" y="2170" fill="{MUTED}" font-size="15" '
                 'font-family="DejaVu Sans, sans-serif">Outline proof / cubic source paths</text>')
    parts.append('</svg>\n')
    destination.write_text("\n".join(parts), encoding="utf-8")


def raster_specimen(font_path, destination):
    from PIL import Image, ImageDraw, ImageFont, features
    if not features.check("raqm"):
        raise RuntimeError("Pillow with RAQM is required for a correctly shaped font proof")
    image = Image.new("RGB", (WIDTH, HEIGHT), GROUND)
    draw = ImageDraw.Draw(image)
    for text, x, baseline, size in ROWS:
        font = ImageFont.truetype(str(font_path), size, layout_engine=ImageFont.Layout.RAQM)
        draw.text((x, baseline), text, font=font, fill=INK, anchor="ls", features=["kern"])
        if text == "splinterm":
            m_start = x + font.getlength(text) - font.getlength("m")
            draw.text((m_start, baseline), "m", font=font, fill=SIGNAL, anchor="ls")
    for text, x, y, size in LABELS:
        label_font = ImageFont.truetype("DejaVuSans.ttf", size)
        draw.text((x, y), text, font=label_font, fill=MUTED, anchor="ls")
    label_font = ImageFont.truetype("DejaVuSans.ttf", 15)
    draw.text((64, 2170), f"Actual {font_path.suffix[1:].upper()} font / FreeType + RAQM rendering",
              font=label_font, fill=MUTED, anchor="ls")
    image.save(destination)


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--font", type=Path, default=build.ROOT / "dist" / f"{build.STEM}.ttf")
    parser.add_argument("--output", type=Path, default=build.ROOT / "dist/specimen.png")
    args = parser.parse_args()
    raster_specimen(args.font, args.output)
    print(f"Rendered {args.font} → {args.output}")


if __name__ == "__main__":
    main()
