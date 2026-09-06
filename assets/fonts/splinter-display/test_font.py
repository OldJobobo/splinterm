"""Non-graphical source, export, shaping, geometry, and raster smoke checks."""

import hashlib
from io import BytesIO
import json
from pathlib import Path
import tempfile
import unittest

from fontTools.pens.boundsPen import BoundsPen
from fontTools.pens.pointInsidePen import PointInsidePen
from fontTools.ttLib import TTFont
from PIL import ImageFont, features
import uharfbuzz as hb

import build
from kerning import KERN, TITLE_KERN
import specimen

EXPECTED = set(range(32, 127)) | {0xA0, 0x2013, 0x2014, 0x2018, 0x2019, 0x201C, 0x201D, 0x2022, 0x2026}


class FontTests(unittest.TestCase):
    def setUp(self):
        self.glyphs = build.load_glyphs()
        self.cmap = {g["codepoint"]: name for name, g in self.glyphs.items() if "codepoint" in g}
        self.temp = tempfile.TemporaryDirectory(dir=build.ROOT)
        self.addCleanup(self.temp.cleanup)
        self.directory = Path(self.temp.name)

    def generate(self, suffix):
        path = self.directory / f"draft.{suffix}"
        build.build_font(self.glyphs, path, ttf=suffix == "ttf")
        return path

    def test_approved_lettering_is_preserved(self):
        seed_path = build.ROOT / "glyphs.json"
        self.assertEqual(hashlib.sha256(seed_path.read_bytes()).hexdigest(),
                         "745c02fad7f9be0b40bda348eb85e681d8084740fc5d90e9d89c6cc7e1be0e9a")
        for name, spec in json.loads(seed_path.read_text(encoding="utf-8")).items():
            self.assertEqual(self.glyphs[name], spec)
        for pair in zip("splinterm", "plinterm"):
            self.assertEqual(KERN[pair], TITLE_KERN[pair])

    def test_source_coverage_is_unique_and_complete(self):
        self.assertEqual(set(self.cmap), EXPECTED)
        self.assertEqual(len(self.glyphs), len(EXPECTED) + 1)
        self.assertEqual(len(self.cmap), len(self.glyphs) - 1)
        for spec in self.glyphs.values():
            commands = build.outline(spec).value
            self.assertEqual(sum(op == "moveTo" for op, _ in commands),
                             sum(op == "closePath" for op, _ in commands))
            self.assertNotIn("endPath", [op for op, _ in commands])

    def test_exports_cmap_names_metrics_and_bounds(self):
        self.generate("otf")
        self.generate("ttf")
        for suffix in ("ttf", "otf", "woff2"):
            with self.subTest(format=suffix), TTFont(self.directory / f"draft.{suffix}") as font:
                self.assertEqual(set(font.getBestCmap()), EXPECTED)
                self.assertEqual(font.getGlyphOrder()[0], ".notdef")
                self.assertEqual(font["name"].getDebugName(1), "Splinter Display Heavy")
                self.assertEqual(font["name"].getDebugName(2), "Regular")
                self.assertEqual(font["name"].getDebugName(16), build.FAMILY)
                self.assertEqual(font["name"].getDebugName(17), "Heavy")
                self.assertEqual(font["name"].getDebugName(6), build.STEM)
                self.assertEqual(font["name"].getDebugName(5), f"Version {build.VERSION}")
                self.assertEqual(float(build.VERSION), float(build.RELEASE))
                self.assertEqual(font["name"].getDebugName(13),
                                 (build.ROOT.parents[2] / "LICENSE").read_text(encoding="utf-8"))
                self.assertEqual(font["OS/2"].usWeightClass, 900)
                self.assertEqual(font["OS/2"].fsType, 0)
                self.assertEqual(font["OS/2"].version, 4)
                self.assertEqual(font["OS/2"].fsSelection, 0xC0)
                self.assertEqual(font["OS/2"].sCapHeight, 720)
                self.assertEqual(font["head"].macStyle, 0)
                self.assertEqual(font["head"].unitsPerEm, 1000)
                self.assertAlmostEqual(font["head"].fontRevision, float(build.VERSION), places=4)
                self.assertEqual(font["post"].isFixedPitch, 0)
                self.assertIn("GPOS", font)
                glyph_set = font.getGlyphSet()
                for name in font.getGlyphOrder():
                    pen = BoundsPen(glyph_set)
                    glyph_set[name].draw(pen)
                    width, bearing = font["hmtx"][name]
                    self.assertGreater(width, 0)
                    if not self.glyphs[name]["contours"]:
                        self.assertIsNone(pen.bounds)
                    else:
                        xmin, ymin, xmax, ymax = pen.bounds
                        self.assertAlmostEqual(bearing, xmin, delta=1)
                        self.assertGreaterEqual(xmin, 0, name)
                        self.assertLessEqual(xmax, width, name)
                        self.assertGreaterEqual(ymin, -240, name)
                        self.assertLessEqual(ymax, 820, name)
                self.assertEqual({font["hmtx"][self.cmap[ord(c)]][0] for c in "0123456789"}, {610})

    def test_counters_and_separate_dots_survive_both_outlines(self):
        probes = [
            ("p", (290, 260), False), ("p", (100, 260), True),
            ("e", (280, 350), False), ("e", (100, 260), True),
            ("i", (100, 660), True), ("i", (100, 560), False),
            ("a", (270, 180), False), ("a", (450, 200), True),
            ("g", (285, 260), False), ("o", (284, 260), False),
            ("A", (360, 350), False), ("A", (360, 200), True),
            ("B", (310, 500), False), ("B", (310, 220), False),
            ("B", (310, 360), True), ("D", (350, 360), False),
            ("O", (387, 360), False), ("P", (330, 475), False),
            ("Q", (387, 360), False), ("R", (330, 480), False),
            ("0", (305, 360), False), ("4", (280, 340), False),
            ("6", (307, 235), False), ("8", (305, 520), False),
            ("8", (305, 222), False), ("9", (303, 485), False),
            ("#", (370, 360), False), ("%", (207, 535), False),
            ("%", (711, 185), False), ("&", (319, 550), False),
            ("&", (320, 210), False), ("@", (568, 346), False),
        ]
        for suffix in ("otf", "ttf"):
            path = self.generate(suffix)
            raster_font = ImageFont.truetype(str(path), 1000)
            with TTFont(path) as font:
                glyph_set = font.getGlyphSet()
                for char, point, ink in probes:
                    name = self.cmap[ord(char)]
                    # Offset the analytic ray off integer curve joins; PointInsidePen
                    # double-counts some quadratic joins (R at y=480, 9 at y=485).
                    # FreeType below independently checks the exact original point.
                    pen = PointInsidePen(glyph_set, (point[0], point[1] + 0.125))
                    glyph_set[name].draw(pen)
                    self.assertEqual(pen.getResult(), ink, (suffix, char, point))
                    mask, (dx, dy) = raster_font.getmask2(char, anchor="ls")
                    px, py = point[0] - dx, -point[1] - dy
                    self.assertEqual(mask[py * mask.size[0] + px] > 0, ink,
                                     ("FreeType", suffix, char, point))

    def test_harfbuzz_maps_every_character_and_kerns_every_pair(self):
        samples = ["".join(chr(c) for c in sorted(EXPECTED)), "splinterm",
                   "AVATAR To Wave. ‘Type’ 09:41", "0123456789"]
        samples.extend(left + right for left, right in KERN)
        for suffix in ("ttf", "otf"):
            path = self.generate(suffix)
            font = hb.Font(hb.Face(path.read_bytes()))
            font.scale = (1000, 1000)
            hb.ot_font_set_funcs(font)
            with TTFont(path) as ttfont:
                for text in samples:
                    for kern in (False, True):
                        buffer = hb.Buffer()
                        buffer.add_str(text)
                        buffer.guess_segment_properties()
                        hb.shape(font, buffer, {"kern": kern})
                        self.assertEqual(len(buffer.glyph_infos), len(text), (suffix, text))
                        self.assertEqual([info.codepoint for info in buffer.glyph_infos],
                                         [ttfont.getGlyphID(self.cmap[ord(c)]) for c in text], text)
                        plain = sum(self.glyphs[self.cmap[ord(c)]]["advance"] for c in text)
                        adjustment = sum(KERN.get(p, 0) for p in zip(text, text[1:])) if kern else 0
                        self.assertEqual(sum(p.x_advance for p in buffer.glyph_positions), plain + adjustment, text)
                buffer = hb.Buffer()
                buffer.add_str("é")
                buffer.guess_segment_properties()
                hb.shape(font, buffer)
                self.assertEqual(buffer.glyph_infos[0].codepoint, ttfont.getGlyphID(".notdef"))

    def test_distributed_files_match_sources(self):
        self.generate("otf")
        self.generate("ttf")
        for suffix in ("otf", "ttf", "woff2"):
            self.assertEqual((self.directory / f"draft.{suffix}").read_bytes(),
                             (build.ROOT / "dist" / f"{build.STEM}.{suffix}").read_bytes())
        proof = self.directory / "specimen.svg"
        specimen.svg_specimen(self.glyphs, proof)
        self.assertEqual(proof.read_bytes(), (build.ROOT / "dist/specimen.svg").read_bytes())

    def test_repeat_builds_are_identical(self):
        for suffix in ("ttf", "otf"):
            path = self.generate(suffix)
            first = path.read_bytes()
            woff = path.with_suffix(".woff2")
            first_woff = woff.read_bytes() if suffix == "ttf" else None
            self.generate(suffix)
            self.assertEqual(first, path.read_bytes())
            if first_woff is not None:
                self.assertEqual(first_woff, woff.read_bytes())

    def test_freetype_renders_every_visible_character(self):
        self.assertTrue(features.check("raqm"))
        for suffix in ("ttf", "otf"):
            path = self.generate(suffix)
            font = ImageFont.truetype(str(path), 160, layout_engine=ImageFont.Layout.RAQM)
            for codepoint in sorted(EXPECTED - {32, 0xA0}):
                mask = font.getmask(chr(codepoint))
                self.assertIsNotNone(mask.getbbox(), (suffix, chr(codepoint)))
        # A browser decodes WOFF2 before shaping; exercise that decoded sfnt too.
        with TTFont(self.directory / "draft.woff2") as font:
            font.flavor = None
            decoded = BytesIO()
            font.save(decoded)
        decoded.seek(0)
        web_font = ImageFont.truetype(decoded, 160)
        self.assertIsNotNone(web_font.getmask("splinterm").getbbox())

    def test_kerned_pairs_do_not_collide_at_display_size(self):
        path = self.generate("ttf")
        with TTFont(path) as font:
            glyph_set = font.getGlyphSet()
            bounds = {}
            for name, glyph in glyph_set.items():
                pen = BoundsPen(glyph_set)
                glyph.draw(pen)
                bounds[name] = pen.bounds
            # Most pairs have disjoint bounding boxes. Check possible overlap at 2-unit intervals.
            for (left, right), kern in KERN.items():
                left_name, right_name = self.cmap[ord(left)], self.cmap[ord(right)]
                lb, rb = bounds[left_name], bounds[right_name]
                shift = font["hmtx"][left_name][0] + kern
                x0, x1 = max(lb[0], rb[0] + shift), min(lb[2], rb[2] + shift)
                y0, y1 = max(lb[1], rb[1]), min(lb[3], rb[3])
                for x in range(int(x0) + 1, int(x1), 2):
                    for y in range(int(y0) + 1, int(y1), 2):
                        lp = PointInsidePen(glyph_set, (x, y))
                        rp = PointInsidePen(glyph_set, (x - shift, y))
                        glyph_set[left_name].draw(lp)
                        glyph_set[right_name].draw(rp)
                        self.assertFalse(lp.getResult() and rp.getResult(), (left + right, x, y))

    def test_proof_rows_fit_and_have_no_vertical_overlap(self):
        cmap = specimen.character_map(self.glyphs)
        previous_bottom = 80
        for text, x, baseline, size in specimen.ROWS:
            ymin, ymax = 0, 0
            for char in text:
                pen = BoundsPen(None)
                build.outline(cmap[char]).replay(pen)
                if pen.bounds:
                    ymin, ymax = min(ymin, pen.bounds[1]), max(ymax, pen.bounds[3])
            self.assertLessEqual(x + specimen.advance(text, cmap) * size / 1000, specimen.WIDTH - 40)
            self.assertGreater(baseline - ymax * size / 1000, previous_bottom, text)
            previous_bottom = baseline - ymin * size / 1000
        self.assertLess(previous_bottom, specimen.HEIGHT - 100)


if __name__ == "__main__":
    unittest.main()
