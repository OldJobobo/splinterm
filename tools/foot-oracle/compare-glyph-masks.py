#!/usr/bin/env python3
"""Compare cell-aligned glyph-mask JSONL emitted by Phase 8.1 probes."""

from __future__ import annotations

import argparse
import json
import sys
from pathlib import Path

GEOMETRY_FIELDS = (
    "font_ascent",
    "font_descent",
    "font_height",
    "decorations",
    "cols",
    "placement",
    "image",
    "advance",
    "ink",
)


def load_records(path: Path, label_prefix: str | None = None) -> dict[str, dict]:
    records: dict[str, dict] = {}
    for line_number, line in enumerate(path.read_text(encoding="utf-8").splitlines(), 1):
        if not line.strip():
            continue
        record = json.loads(line)
        codepoint = record.get("codepoint")
        label = record.get("label")
        if not isinstance(codepoint, int):
            raise ValueError(f"{path}:{line_number}: missing integer codepoint")
        if not isinstance(label, str) or not label:
            raise ValueError(f"{path}:{line_number}: missing non-empty label")
        if label_prefix is not None and not label.startswith(label_prefix):
            continue
        if label in records:
            raise ValueError(f"{path}:{line_number}: duplicate label {label!r}")
        mask = record.get("alpha_hex")
        image = record.get("image", {})
        expected_bytes = image.get("width", 0) * image.get("height", 0)
        if not isinstance(mask, str) or len(mask) != expected_bytes * 2:
            raise ValueError(
                f"{path}:{line_number}: alpha_hex length does not match image dimensions"
            )
        records[label] = record
    return records


def write_heatmap(path: Path, width: int, height: int, differences: bytes) -> None:
    path.write_bytes(f"P5\n{width} {height}\n255\n".encode("ascii") + differences)


def compare(reference: dict[str, dict], actual: dict[str, dict], output_dir: Path) -> dict:
    output_dir.mkdir(parents=True, exist_ok=True)
    missing = sorted(set(reference) - set(actual))
    unexpected = sorted(set(actual) - set(reference))
    glyphs = []

    for label in sorted(set(reference) & set(actual)):
        expected = reference[label]
        observed = actual[label]
        codepoint = expected["codepoint"]
        geometry_mismatches = [
            field for field in GEOMETRY_FIELDS if expected.get(field) != observed.get(field)
        ]
        expected_mask = bytes.fromhex(expected["alpha_hex"])
        observed_mask = bytes.fromhex(observed["alpha_hex"])
        if len(expected_mask) == len(observed_mask):
            differences = bytes(abs(left - right) for left, right in zip(expected_mask, observed_mask))
            mismatch_count = sum(value != 0 for value in differences)
            maximum_delta = max(differences, default=0)
        else:
            differences = b""
            mismatch_count = max(len(expected_mask), len(observed_mask))
            maximum_delta = 255

        heatmap = None
        image = expected["image"]
        if differences and mismatch_count:
            heatmap = f"U+{codepoint:04X}.pgm"
            write_heatmap(output_dir / heatmap, image["width"], image["height"], differences)

        glyphs.append(
            {
                "label": label,
                "codepoint": codepoint,
                "unicode": f"U+{codepoint:04X}",
                "geometry_mismatches": geometry_mismatches,
                "mismatch_pixels": mismatch_count,
                "maximum_alpha_delta": maximum_delta,
                "heatmap": heatmap,
                "pass": not geometry_mismatches and mismatch_count == 0,
            }
        )

    summary = {
        "reference_glyphs": len(reference),
        "actual_glyphs": len(actual),
        "missing": missing,
        "unexpected": unexpected,
        "passing": sum(item["pass"] for item in glyphs),
        "failing": sum(not item["pass"] for item in glyphs) + len(missing) + len(unexpected),
        "glyphs": glyphs,
    }
    (output_dir / "comparison.json").write_text(
        json.dumps(summary, indent=2, sort_keys=True) + "\n", encoding="utf-8"
    )
    return summary


def main() -> int:
    parser = argparse.ArgumentParser()
    parser.add_argument("--reference", required=True, type=Path)
    parser.add_argument("--actual", required=True, type=Path)
    parser.add_argument("--output-dir", required=True, type=Path)
    parser.add_argument(
        "--label-prefix", help="compare only records whose labels start with this value"
    )
    args = parser.parse_args()

    try:
        summary = compare(
            load_records(args.reference, args.label_prefix),
            load_records(args.actual, args.label_prefix),
            args.output_dir,
        )
    except (OSError, ValueError, json.JSONDecodeError) as error:
        print(f"glyph comparison failed: {error}", file=sys.stderr)
        return 2

    print(
        f"glyphs: {summary['passing']} pass, {summary['failing']} fail "
        f"({summary['reference_glyphs']} reference, {summary['actual_glyphs']} actual)"
    )
    return 1 if summary["failing"] else 0


if __name__ == "__main__":
    raise SystemExit(main())
