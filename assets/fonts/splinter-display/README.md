# Splinter Display Heavy

An original heavy display typeface built around the approved Splinterm lettering:
rounded mass, tight spacing, and angular cuts in the `p` descender and `i` dot.
The founding nine glyphs and the title's kerning remain unchanged.

**Font asset v0.1:** a usable English display alphabet, not a terminal/coding font.
Use it for titles, branding, posters, and short labels rather than small body text.

This version belongs to the font, not the Splinterm application. OpenType metadata
spells it `0.100` (numerically 0.1); filenames remain stable for consumers. See
[CHANGELOG.md](CHANGELOG.md) for the asset release history.

## Try it

Open [`viewer.html`](viewer.html) in a browser to type your own text, adjust its
size, and download the font files. The viewer loads the actual local WOFF2 and
reports loading failures and unsupported characters rather than hiding fallback.
Nothing is installed automatically.

If your browser blocks local font files, serve only this directory:

```sh
python3 -m http.server 8765 --bind 127.0.0.1 --directory assets/fonts/splinter-display
```

Then open `http://127.0.0.1:8765/viewer.html`. Stop the server with Ctrl-C.

## Font files

- [`dist/SplinterDisplay-Heavy.ttf`](dist/SplinterDisplay-Heavy.ttf): TrueType outlines for desktop use.
- [`dist/SplinterDisplay-Heavy.otf`](dist/SplinterDisplay-Heavy.otf): CFF outlines for desktop use.
- [`dist/SplinterDisplay-Heavy.woff2`](dist/SplinterDisplay-Heavy.woff2): compressed TrueType web font.
- [`dist/specimen.png`](dist/specimen.png): actual TTF proof rendered by FreeType + RAQM.
- [`dist/specimen.svg`](dist/specimen.svg): scalable proof of the cubic source outlines.

Install **either TTF or OTF, not both**: they deliberately share family and style
identities. The typographic family is **Splinter Display**, style **Heavy**;
legacy applications may show **Splinter Display Heavy / Regular**. Weight is 900.
The old `SplinterDisplayDraft-Heavy` prototype is superseded by these files.

The font has 104 mapped characters (plus `.notdef`):

- All 95 printable ASCII characters: A–Z, a–z, 0–9, space, and punctuation.
- Nonbreaking space, `‘ ’ “ ” – — … •`.
- Proportional letter widths and tabular lining digits (610 units each).
- 218 explicit GPOS kerning pairs, including the approved title's eight pairs.

There are no accented letters, non-Latin scripts, ligatures, extra weights,
italics, or hinting. Unsupported characters use an application's fallback or
`.notdef`. Amber lettering is styling only; every glyph is monochrome.

For web use, without the old title's additional negative letter spacing:

```css
@font-face {
  font-family: 'Splinter Display';
  src: url('./SplinterDisplay-Heavy.woff2') format('woff2');
  font-style: normal;
  font-weight: 900;
  font-display: swap;
}
.title {
  font-family: 'Splinter Display', sans-serif;
  font-weight: 900;
  font-synthesis: none;
  font-kerning: normal;
  letter-spacing: 0;
}
```

This branch does not change the Splinterm website or install any font.

## Editable source

- `glyphs.json`: the unchanged approved `e i l m n p r s t` and space.
- `alphabet.py`: original companion outlines and explicit mirrored/rotated derivatives.
- `kerning.py`: optical pair adjustments in font units.
- `build.py`: deterministic FontTools compiler for all three formats.
- `specimen.py`: shared layout for outline and actual-font proofs.
- `viewer.html`: standalone, editable browser specimen.
- `test_font.py`: source, build, layout, and renderer checks.
- `test_viewer.cjs`: viewer controls, coverage warnings, and loading-state checks.

Contours use cubic Bézier paths in a 1000-unit em with positive y upward.
X-height is 520, cap height 720, ascender 740; line metrics are +820/−240 with
no extra line gap. Rounded forms overshoot the baseline and nominal height.
Each glyph explicitly identifies counters; the compiler normalizes winding and
converts cubics to quadratics for TrueType with a 0.5-unit approximation tolerance
before integer coordinate rounding.
No existing font outlines are imported, traced, or modified.

## Build and validate

From a dedicated task worktree's repository root, with Python 3 and `uv`:

```sh
UV_CACHE_DIR="$PWD/.font-cache" uv venv --python /usr/bin/python3 .font-venv
UV_CACHE_DIR="$PWD/.font-cache" uv pip install --python .font-venv/bin/python \
  -r assets/fonts/splinter-display/requirements.txt
.font-venv/bin/python assets/fonts/splinter-display/build.py
.font-venv/bin/python assets/fonts/splinter-display/specimen.py
.font-venv/bin/python -m unittest discover -s assets/fonts/splinter-display -v
node --test assets/fonts/splinter-display/test_viewer.cjs
git diff --check
```

Raster proof generation needs Pillow with RAQM and the system's DejaVu Sans
for utility labels. The font itself does not depend on DejaVu Sans. To inspect
CFF rendering too, pass `--font dist/SplinterDisplay-Heavy.otf` and a separate
`--output` path to `specimen.py` (paths resolve from the invoking directory).

The checks cover all exported cmaps, names, style bits, vertical bounds, side
bearings, counters, separate dots, every mapped character and kerning pair in
HarfBuzz, fixed-width digits, preservation of the approved lettering, deterministic
font builds, and generated-output/source equality. FreeType renders every visible
character in both desktop formats and a decoded WOFF2 sample. Pair-overlap probes
sample potential collisions at 2-unit intervals; they are not an exhaustive
mathematical intersection proof. Proof rows are checked for fit and overlap.

The font and SVG builds are repeatable with pinned dependencies. PNG pixels can
vary across renderer/system-label-font versions. Native application installation,
Windows/macOS behavior, and small-text hinting quality are not certified by these
checks. Standalone release, installation, and publication are separate steps.

This directory follows the repository's existing [MIT license](../../../LICENSE).
The full license text is also embedded in the font's name table. Include that
license when redistributing; no third-party font license is introduced.
