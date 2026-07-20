#!/usr/bin/env python3

import importlib.util
import tempfile
import unittest
from pathlib import Path

MODULE_PATH = Path(__file__).with_name("compare-glyph-masks.py")
SPEC = importlib.util.spec_from_file_location("compare_glyph_masks", MODULE_PATH)
assert SPEC and SPEC.loader
MODULE = importlib.util.module_from_spec(SPEC)
SPEC.loader.exec_module(MODULE)


def record(mask: str = "00ff", *, x: int = 0) -> dict:
    return {
        "label": "ASCII-U+0041",
        "codepoint": 65,
        "font_ascent": 10,
        "font_descent": 3,
        "font_height": 13,
        "cols": 1,
        "placement": {"x": x, "y": 10},
        "image": {"width": 2, "height": 1},
        "advance": {"x": 8, "y": 0},
        "ink": {"left": 1, "top": 0, "right": 2, "bottom": 1},
        "alpha_hex": mask,
    }


class ComparisonTests(unittest.TestCase):
    def test_identical_glyph_passes(self) -> None:
        with tempfile.TemporaryDirectory() as directory:
            summary = MODULE.compare(
                {"ASCII-U+0041": record()}, {"ASCII-U+0041": record()}, Path(directory)
            )
            self.assertEqual(summary["passing"], 1)
            self.assertEqual(summary["failing"], 0)
            self.assertFalse((Path(directory) / "U+0041.pgm").exists())

    def test_mask_and_geometry_mismatch_produce_heatmap(self) -> None:
        with tempfile.TemporaryDirectory() as directory:
            summary = MODULE.compare(
                {"ASCII-U+0041": record()},
                {"ASCII-U+0041": record("ffff", x=1)},
                Path(directory),
            )
            glyph = summary["glyphs"][0]
            self.assertEqual(glyph["geometry_mismatches"], ["placement"])
            self.assertEqual(glyph["mismatch_pixels"], 1)
            self.assertEqual(glyph["maximum_alpha_delta"], 255)
            self.assertTrue((Path(directory) / "U+0041.pgm").exists())

    def test_color_channel_mismatch_fails_even_when_alpha_matches(self) -> None:
        with tempfile.TemporaryDirectory() as directory:
            expected = {**record(), "rgba_hex": "00000000ffffffff"}
            observed = {**record(), "rgba_hex": "01000000ffffffff"}
            summary = MODULE.compare(
                {"ASCII-U+0041": expected},
                {"ASCII-U+0041": observed},
                Path(directory),
            )
            glyph = summary["glyphs"][0]
            self.assertEqual(glyph["mismatch_pixels"], 0)
            self.assertEqual(glyph["color_mismatch_pixels"], 1)
            self.assertEqual(glyph["maximum_color_delta"], 1)
            self.assertEqual(summary["failing"], 1)

    def test_lane_owned_geometry_field_can_be_explicitly_ignored(self) -> None:
        with tempfile.TemporaryDirectory() as directory:
            observed = record()
            observed["advance"] = {"x": 7, "y": 0}
            summary = MODULE.compare(
                {"ASCII-U+0041": record()},
                {"ASCII-U+0041": observed},
                Path(directory),
                frozenset({"advance"}),
            )
            self.assertEqual(summary["passing"], 1)
            self.assertEqual(summary["glyphs"][0]["geometry_mismatches"], [])

    def test_missing_and_unexpected_glyphs_fail(self) -> None:
        with tempfile.TemporaryDirectory() as directory:
            summary = MODULE.compare(
                {"ASCII-U+0041": record()},
                {"ASCII-U+0042": {**record(), "label": "ASCII-U+0042", "codepoint": 66}},
                Path(directory),
            )
            self.assertEqual(summary["missing"], ["ASCII-U+0041"])
            self.assertEqual(summary["unexpected"], ["ASCII-U+0042"])
            self.assertEqual(summary["failing"], 2)


if __name__ == "__main__":
    unittest.main()
