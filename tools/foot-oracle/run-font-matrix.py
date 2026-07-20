#!/usr/bin/env python3
"""Run the source-first Phase 8.1 face/size/scale headless matrix."""

from __future__ import annotations

import argparse
import hashlib
import json
import os
import subprocess
import sys
from dataclasses import dataclass
from pathlib import Path
from typing import Any

ROOT = Path(__file__).resolve().parents[2]
TOOLS = ROOT / "tools/foot-oracle"
STYLES = ("Regular", "Bold", "Italic", "Bold Italic")
LOGICAL_SIZES = (6, 12, 22, 32, 48, 96)
SCALES_120 = (120, 150, 180, 240)


@dataclass(frozen=True)
class MatrixCase:
    style: str
    logical_size: int
    scale_120: int

    @property
    def effective_size(self) -> float:
        return self.logical_size * self.scale_120 / 120

    @property
    def identifier(self) -> str:
        style = self.style.lower().replace(" ", "-")
        return f"{style}-{self.logical_size}px-{self.scale_120}"


def matrix_cases() -> list[MatrixCase]:
    return [
        MatrixCase(style, logical_size, scale)
        for style in STYLES
        for logical_size in LOGICAL_SIZES
        for scale in SCALES_120
    ]


def run(
    command: list[str | Path],
    *,
    env: dict[str, str] | None = None,
    stdout: Path | None = None,
) -> subprocess.CompletedProcess[str]:
    process_env = os.environ.copy()
    process_env.update(env or {})
    if stdout is None:
        return subprocess.run(
            [str(part) for part in command],
            cwd=ROOT,
            env=process_env,
            text=True,
            capture_output=True,
            check=False,
        )
    with stdout.open("w", encoding="utf-8") as output:
        return subprocess.run(
            [str(part) for part in command],
            cwd=ROOT,
            env=process_env,
            text=True,
            stdout=output,
            stderr=subprocess.PIPE,
            check=False,
        )


def require_success(result: subprocess.CompletedProcess[str], label: str) -> None:
    if result.returncode != 0:
        detail = result.stderr.strip() or result.stdout.strip() or "no diagnostic"
        raise RuntimeError(f"{label} failed: {detail}")


def records_by_label(path: Path) -> dict[str, dict[str, Any]]:
    records: dict[str, dict[str, Any]] = {}
    for line in path.read_text(encoding="utf-8").splitlines():
        record = json.loads(line)
        if not isinstance(record, dict) or not isinstance(record.get("label"), str):
            raise RuntimeError(f"{path} contains an invalid evidence record")
        records[record["label"]] = record
    if not records:
        raise RuntimeError(f"{path} contains no evidence records")
    return records


def first_record(path: Path) -> dict[str, Any]:
    return next(iter(records_by_label(path).values()))


def sha256(path: Path) -> str:
    digest = hashlib.sha256()
    with path.open("rb") as source:
        for chunk in iter(lambda: source.read(1024 * 1024), b""):
            digest.update(chunk)
    return digest.hexdigest()


def resolve_pattern(pattern: str, label: str) -> dict[str, Any]:
    result = run(["fc-match", "-f", "%{file}\n%{index}\n%{family}\n%{style}\n", pattern])
    require_success(result, f"resolve {label}")
    lines = result.stdout.splitlines()
    if len(lines) < 4:
        raise RuntimeError(f"fc-match omitted identity for {label}")
    path = Path(lines[0])
    if not path.is_file():
        raise RuntimeError(f"resolved face is not a file: {path}")
    return {
        "pattern": pattern,
        "path": str(path),
        "index": int(lines[1]),
        "family": lines[2],
        "style": lines[3],
        "sha256": sha256(path),
    }


def resolve_face(case: MatrixCase) -> dict[str, Any]:
    pattern = (
        "JetBrains Mono Nerd Font:"
        f"style={case.style}:pixelsize={case.effective_size:g}"
    )
    return resolve_pattern(pattern, case.identifier)


def resolve_cjk(case: MatrixCase) -> dict[str, Any]:
    pattern = f"Noto Sans CJK JP:pixelsize={case.effective_size:g}"
    return resolve_pattern(pattern, f"{case.identifier} CJK fallback")


def resolve_emoji(case: MatrixCase) -> dict[str, Any]:
    pattern = f"Noto Color Emoji:pixelsize={case.effective_size:g}"
    return resolve_pattern(pattern, f"{case.identifier} emoji fallback")


def validate_actual_identity(
    path: Path, expected: dict[str, Any], label: str = "ASCII-U+0020"
) -> None:
    record = records_by_label(path).get(label)
    if record is None:
        raise RuntimeError(f"{path}: missing identity record {label}")
    observed = (record.get("font_path"), record.get("font_index"))
    wanted = (expected["path"], expected["index"])
    if observed != wanted:
        raise RuntimeError(f"{path}: face identity {observed!r} != {wanted!r}")


def compare(
    reference: Path,
    actual: Path,
    output: Path,
    label_prefix: str,
    ignored_geometry_fields: tuple[str, ...] = (),
) -> dict[str, Any]:
    command: list[str | Path] = [
        sys.executable,
        TOOLS / "compare-glyph-masks.py",
        "--reference",
        reference,
        "--actual",
        actual,
        "--label-prefix",
        label_prefix,
        "--output-dir",
        output,
    ]
    for field in ignored_geometry_fields:
        command.extend(["--ignore-geometry-field", field])
    result = run(command)
    summary_path = output / "comparison.json"
    if not summary_path.is_file():
        require_success(result, f"compare {actual.name}")
        raise RuntimeError(f"comparator omitted {summary_path}")
    summary = json.loads(summary_path.read_text(encoding="utf-8"))
    if result.returncode != 0 or summary.get("failing") != 0:
        raise RuntimeError(result.stdout.strip() or f"comparison failed for {actual}")
    return summary


def capture_case(
    case: MatrixCase,
    case_dir: Path,
    *,
    fcft_binary: Path,
    first_reference: bool,
    freetype_binary: Path,
    production_binary: Path,
) -> dict[str, Any]:
    case_dir.mkdir(parents=True, exist_ok=False)
    effective = f"{case.effective_size:g}"
    environment = {
        "SPLINTERM_EVIDENCE_FONT_SIZE": effective,
        "SPLINTERM_EVIDENCE_FONT_STYLE": case.style,
    }
    reference = case_dir / "fcft-reference.jsonl"
    if first_reference:
        result = run([TOOLS / "run-fcft-mask-probe.sh"], env=environment, stdout=reference)
    else:
        result = run([fcft_binary], env=environment, stdout=reference)
    require_success(result, f"fcft capture {case.identifier}")

    isolated = case_dir / "freetype-actual.jsonl"
    result = run([freetype_binary], env=environment, stdout=isolated)
    require_success(result, f"FreeType capture {case.identifier}")

    production = case_dir / "production-actual.jsonl"
    production_environment = {
        **environment,
        "SPLINTERM_EVIDENCE_LOGICAL_FONT_SIZE": str(case.logical_size),
        "SPLINTERM_EVIDENCE_SCALE_120": str(case.scale_120),
    }
    result = run([production_binary], env=production_environment, stdout=production)
    require_success(result, f"production capture {case.identifier}")

    identity = resolve_face(case)
    fallback_identity = resolve_cjk(case)
    emoji_identity = resolve_emoji(case)
    validate_actual_identity(isolated, identity)
    validate_actual_identity(production, identity)
    validate_actual_identity(isolated, fallback_identity, "CJK")
    validate_actual_identity(production, fallback_identity, "CJK")
    validate_actual_identity(production, emoji_identity, "emoji")
    validate_actual_identity(isolated, identity, "combining-0")
    validate_actual_identity(production, identity, "combining-0")
    isolated_summary = compare(reference, isolated, case_dir / "freetype-diff", "ASCII-U+")
    production_summary = compare(
        reference, production, case_dir / "production-diff", "ASCII-U+"
    )
    isolated_cjk = compare(reference, isolated, case_dir / "freetype-cjk-diff", "CJK")
    production_cjk = compare(
        reference, production, case_dir / "production-cjk-diff", "CJK"
    )
    isolated_combining = compare(
        reference,
        isolated,
        case_dir / "freetype-combining-diff",
        "combining-",
        ("advance",),
    )
    production_combining = compare(
        reference, production, case_dir / "production-combining-diff", "combining-"
    )
    production_emoji = compare(
        reference, production, case_dir / "production-emoji-diff", "emoji"
    )
    return {
        "id": case.identifier,
        "style": case.style,
        "logical_size_px": case.logical_size,
        "scale_120": case.scale_120,
        "effective_size_px": case.effective_size,
        "face": identity,
        "cjk_fallback": fallback_identity,
        "emoji_fallback": emoji_identity,
        "reference_font": first_record(reference).get("font"),
        "isolated_passing": isolated_summary["passing"],
        "production_passing": production_summary["passing"],
        "cjk_isolated_passing": isolated_cjk["passing"],
        "cjk_production_passing": production_cjk["passing"],
        "combining_isolated_passing": isolated_combining["passing"],
        "combining_production_passing": production_combining["passing"],
        "emoji_production_passing": production_emoji["passing"],
        "glyphs": (
            isolated_summary["reference_glyphs"]
            + isolated_cjk["reference_glyphs"]
            + isolated_combining["reference_glyphs"]
            + production_emoji["reference_glyphs"]
        ),
        "exact": True,
    }


def write_summary(output: Path, cases: list[dict[str, Any]], error: str | None = None) -> None:
    payload = {
        "schema": "splinterm.font-matrix.v1",
        "source_first": True,
        "declared_case_count": len(matrix_cases()),
        "completed_case_count": len(cases),
        "exact": error is None and all(case["exact"] for case in cases),
        "error": error,
        "cases": cases,
    }
    (output / "summary.json").write_text(
        json.dumps(payload, indent=2, sort_keys=True) + "\n", encoding="utf-8"
    )


def main() -> int:
    parser = argparse.ArgumentParser()
    parser.add_argument("output", type=Path)
    parser.add_argument("--case", choices=[case.identifier for case in matrix_cases()])
    args = parser.parse_args()
    args.output.mkdir(parents=True, exist_ok=False)

    selected = [case for case in matrix_cases() if args.case in (None, case.identifier)]
    build = run(
        [
            "cargo",
            "build",
            "-q",
            "-p",
            "splinterm",
            "--example",
            "ascii-freetype-evidence",
            "--example",
            "ascii-production-evidence",
        ]
    )
    require_success(build, "build Rust evidence exporters")
    fcft_binary = Path(
        os.environ.get("FOOT_REFERENCE_BUILD_DIR", "/tmp/splinterm-foot-build")
    ) / "fcft-mask-probe"
    freetype_binary = ROOT / "target/debug/examples/ascii-freetype-evidence"
    production_binary = ROOT / "target/debug/examples/ascii-production-evidence"

    completed: list[dict[str, Any]] = []
    try:
        for index, case in enumerate(selected):
            result = capture_case(
                case,
                args.output / case.identifier,
                fcft_binary=fcft_binary,
                first_reference=index == 0,
                freetype_binary=freetype_binary,
                production_binary=production_binary,
            )
            completed.append(result)
            write_summary(args.output, completed)
            print(f"PASS {case.identifier} ({result['glyphs']} glyphs)", flush=True)
    except (OSError, ValueError, RuntimeError, json.JSONDecodeError) as error:
        write_summary(args.output, completed, str(error))
        print(f"font matrix failed: {error}", file=sys.stderr)
        return 1

    print(f"Font matrix: {len(completed)}/{len(selected)} exact")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
