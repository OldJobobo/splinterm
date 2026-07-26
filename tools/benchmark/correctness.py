"""Non-graphical correctness evidence collection for Splinterbench."""

from __future__ import annotations

import hashlib
import json
import pathlib
import subprocess
import sys
from collections.abc import Callable, Sequence
from typing import Any

ROOT = pathlib.Path(__file__).resolve().parents[2]
PINNED_FOOT = "3c5b584b0eafa772eb4376fb6eaf6643399e190e"
TERMINALS = ("splinterm", "foot", "kitty", "ghostty", "alacritty")

SEMANTIC_FIXTURES = ROOT / "fixtures/terminal/v1"
ORACLE = ROOT / "tools/foot-oracle"
FINAL_BUFFER_EVIDENCE = (
    ("base-final-buffer", ROOT / "docs/spikes/artifacts/0017/slice1-final-buffer/summary.json"),
    ("decoration-cursor", ROOT / "docs/spikes/artifacts/0017/slice3-decoration-cursor/summary.json"),
    ("font-matrix", ROOT / "docs/spikes/artifacts/0017/slice4-font-matrix-final/summary.json"),
    ("scale-fallback-integration", ROOT / "docs/spikes/artifacts/0017/slice4-graphical-final/summary.json"),
)
EXTERNAL_EVIDENCE = {
    "output-marker": ROOT / "docs/benchmarks/artifacts/2026-07-23-five-terminal-output/matrix.json",
    "settled-resize": ROOT / "docs/benchmarks/artifacts/2026-07-23-five-terminal-resize/matrix.json",
    "child-exit": ROOT / "docs/benchmarks/artifacts/2026-07-23-five-terminal-lifecycle/matrix.json",
}

Run = Callable[[Sequence[str]], subprocess.CompletedProcess[str]]


def relative(path: pathlib.Path) -> str:
    return str(path.relative_to(ROOT))


def sha256(path: pathlib.Path) -> str:
    digest = hashlib.sha256()
    with path.open("rb") as stream:
        for chunk in iter(lambda: stream.read(1024 * 1024), b""):
            digest.update(chunk)
    return digest.hexdigest()


def load_json(path: pathlib.Path) -> dict[str, Any]:
    value = json.loads(path.read_text(encoding="utf-8"))
    if not isinstance(value, dict):
        raise ValueError(f"{relative(path)} must contain a JSON object")
    return value


def collect_semantic_fixtures() -> dict[str, Any]:
    fixtures = []
    for path in sorted(SEMANTIC_FIXTURES.glob("*.json")):
        value = load_json(path)
        reference = value.get("reference", {})
        if reference.get("commit") != PINNED_FOOT:
            raise ValueError(f"{relative(path)} does not use the pinned Foot commit")
        if reference.get("verification") != "oracle_verified":
            raise ValueError(f"{relative(path)} is not oracle verified")
        if value.get("intentional_divergences"):
            raise ValueError(f"{relative(path)} contains an unreported parity divergence")
        fixtures.append(
            {
                "id": value["id"],
                "path": relative(path),
                "sha256": sha256(path),
                "verification": reference["verification"],
            }
        )
    if len(fixtures) != 5:
        raise ValueError(f"expected 5 v1 semantic fixtures, found {len(fixtures)}")
    return {
        "status": "covered",
        "fixture_count": len(fixtures),
        "fixtures": fixtures,
        "rust_test": "crates/splinterm-terminal/tests/oracle_fixtures.rs",
        "scope": "Canonical JSON is converted by a checked generator into dependency-free Rust vectors covering normalized visible rows, cursor/deferred-wrap state, attributes, events, and whole/bytewise/split/deterministic chunking.",
    }


def collect_final_buffer_evidence() -> list[dict[str, Any]]:
    evidence = []
    for lane, path in FINAL_BUFFER_EVIDENCE:
        value = load_json(path)
        cases = value.get("cases")
        exact = value.get("exact") is True
        if not isinstance(cases, list) or not cases or not exact:
            raise ValueError(f"{relative(path)} is not complete exact evidence")
        if any(case.get("exact") is not True for case in cases):
            raise ValueError(f"{relative(path)} contains a non-exact case")
        evidence.append(
            {
                "lane": lane,
                "path": relative(path),
                "sha256": sha256(path),
                "schema": value.get("schema"),
                "case_count": len(cases),
                "exact": True,
            }
        )
    return evidence


def collect_external_observations() -> list[dict[str, Any]]:
    observations = []
    for observation, path in EXTERNAL_EVIDENCE.items():
        value = load_json(path)
        if value.get("valid") is not True:
            raise ValueError(f"{relative(path)} is not valid benchmark evidence")
        expected = value.get("expected_measured_cases")
        completed = value.get("completed_measured_cases")
        if not isinstance(expected, int) or completed != expected:
            raise ValueError(f"{relative(path)} is incomplete")
        summary = value.get("summary")
        if not isinstance(summary, dict):
            raise ValueError(f"{relative(path)} has no summary")
        observed_terminals = set(summary)
        if observation == "output-marker":
            observed_terminals = set.intersection(
                *(set(case) for case in summary.values() if isinstance(case, dict))
            )
        if observed_terminals != set(TERMINALS):
            raise ValueError(f"{relative(path)} does not cover all terminals")
        observations.append(
            {
                "observation": observation,
                "path": relative(path),
                "sha256": sha256(path),
                "terminals": list(TERMINALS),
                "measured_cases": completed,
                "status": "observed",
                "claim_limit": {
                    "output-marker": "A high-contrast completion marker became externally visible; screenshot polling does not prove intervening cell content.",
                    "settled-resize": "The compositor reported every requested settled geometry; private grid/reflow state was not inspected.",
                    "child-exit": "Child exit and window/process lifecycle were externally observed; retained private terminal state was not inspected.",
                }[observation],
            }
        )
    return observations


def default_run(command: Sequence[str]) -> subprocess.CompletedProcess[str]:
    return subprocess.run(
        list(command), cwd=ROOT, text=True, capture_output=True, check=False, timeout=300
    )


def run_checks(run: Run = default_run) -> list[dict[str, Any]]:
    foot_tests = sorted(
        str(path.relative_to(ROOT))
        for path in ORACLE.glob("test_*.py")
        if path.name != "test_check_provenance.py"
    )
    commands = (
        ("semantic-vector-sync", True, [sys.executable, "tools/benchmark/generate-semantic-fixture-vectors.py", "--check"]),
        ("semantic-fixture-validation", True, [sys.executable, "tools/foot-oracle/validate-fixtures.py"]),
        ("oracle-comparator-tests", True, [sys.executable, "-m", "pytest", "-q", *foot_tests]),
        ("terminal-correctness-tests", True, ["cargo", "test", "-q", "-p", "splinterm-terminal"]),
        ("workspace-oracle-provenance", False, [sys.executable, "tools/foot-oracle/check-provenance.py", "--portable"]),
    )
    results = []
    for check, required, command in commands:
        completed = run(command)
        output = "\n".join(
            line for line in (completed.stdout + completed.stderr).splitlines() if line.strip()
        )[-4000:]
        results.append(
            {
                "check": check,
                "required": required,
                "command": list(command),
                "returncode": completed.returncode,
                "status": "passed" if completed.returncode == 0 else "failed",
                "output": output,
            }
        )
    return results


def repository_state(run: Run = default_run) -> dict[str, Any]:
    revision = run(["git", "rev-parse", "HEAD"])
    status = run(["git", "status", "--porcelain"])
    if revision.returncode != 0 or status.returncode != 0:
        raise ValueError("cannot inspect repository state")
    return {"revision": revision.stdout.strip(), "dirty": bool(status.stdout.strip())}


def feature_coverage() -> list[dict[str, Any]]:
    terminal_tests = "crates/splinterm-terminal/src/terminal.rs"
    snapshot_tests = "crates/splinterm-terminal/tests/snapshot.rs"
    return [
        {"feature": "unicode-width-combining-emoji", "status": "covered", "evidence": [terminal_tests, "docs/spikes/artifacts/0017/slice4-font-matrix-final/summary.json"]},
        {"feature": "sgr", "status": "covered", "evidence": [terminal_tests, "fixtures/terminal/v1/sgr-basic.json"]},
        {"feature": "alternate-screen", "status": "covered", "evidence": [terminal_tests, snapshot_tests]},
        {"feature": "cursor-and-erase", "status": "covered", "evidence": [terminal_tests, "fixtures/terminal/v1/cursor-position.json", "fixtures/terminal/v1/erase-line.json"]},
        {"feature": "resize-and-reflow", "status": "covered", "evidence": ["crates/splinterm-terminal/src/grid/reflow.rs", snapshot_tests]},
        {"feature": "title-and-pty-replies", "status": "covered", "evidence": [terminal_tests]},
        {"feature": "malformed-sequence-recovery", "status": "covered", "evidence": [terminal_tests]},
        {"feature": "parser-fuzzing", "status": "available-not-run", "evidence": ["fuzz/fuzz_targets/terminal_advance.rs"], "note": "Target availability is not reported as a fuzz pass; bounded fuzz runs must be recorded separately."},
        {"feature": "hyperlinks-osc-8", "status": "unsupported", "evidence": [terminal_tests], "note": "OSC 8 has no terminal handler; unsupported is not scored as a failed performance case."},
    ]


def capability_matrix() -> list[dict[str, Any]]:
    rows = []
    for capability, splinterm_status, evidence, note in (
        ("sixel", "partial", ["docs/roadmap.md", "crates/splinterm-terminal/src/image/sixel.rs"], "Streaming decode, placement modes, and queries exist; overlap/reflow/configuration/fuzz/full differential closure remains in progress."),
        ("kitty-graphics", "unsupported", ["docs/roadmap.md", "crates/splinterm-terminal/src/vt/mod.rs"], "No Kitty APC protocol handler is implemented."),
        ("iterm2-images", "unsupported", ["docs/roadmap.md"], "Explicitly deferred."),
    ):
        statuses = {terminal: "unknown" for terminal in TERMINALS}
        statuses["splinterm"] = splinterm_status
        rows.append({"capability": capability, "statuses": statuses, "evidence": evidence, "note": note + " Other terminals remain unknown because this report does not inspect private state or infer support from availability."})
    return rows


def build_report(run: Run = default_run) -> dict[str, Any]:
    semantic = collect_semantic_fixtures()
    final_buffer = collect_final_buffer_evidence()
    external = collect_external_observations()
    checks = run_checks(run)
    report = {
        "schema": "splinterm.benchmark.correctness.v1",
        "valid": all(
            check["status"] == "passed" for check in checks if check["required"]
        ),
        "repository": repository_state(run),
        "oracle": {"name": "foot", "version": "1.27.0", "commit": PINNED_FOOT, "authority": "behavioral-reference"},
        "semantic_fixtures": semantic,
        "final_buffer_evidence": final_buffer,
        "checks": checks,
        "feature_coverage": feature_coverage(),
        "fuzzing": {"target": "fuzz/fuzz_targets/terminal_advance.rs", "status": "available-not-run", "recorded_duration_seconds": None},
        "capability_matrix": capability_matrix(),
        "external_observations": external,
        "claim_policy": "Correctness is independent from speed. Exact Splinterm/Foot evidence is reported only where semantic or pixel comparison exists. Five-terminal private state is not inferred from portable external observations.",
    }
    return report


def render_markdown(report: dict[str, Any]) -> str:
    semantic = report["semantic_fixtures"]
    lines = [
        "# Splinterbench correctness report",
        "",
        f"Overall non-graphical validation: **{'PASS' if report['valid'] else 'FAIL'}**  ",
        f"Repository revision: `{report['repository']['revision']}` ({'dirty' if report['repository']['dirty'] else 'clean'} worktree)  ",
        f"Behavioral oracle: Foot 1.27.0 `{report['oracle']['commit']}`",
        "",
        "Correctness is reported separately from performance. Portable observations for other terminals do not expose or prove private terminal state.",
        "",
        "## Oracle parity",
        "",
        f"- Semantic fixtures: **{semantic['fixture_count']}/{semantic['fixture_count']} covered** by the Rust fixture consumer, including chunking invariance.",
    ]
    for lane in report["final_buffer_evidence"]:
        lines.append(f"- {lane['lane']}: **{lane['case_count']}/{lane['case_count']} exact** (`{lane['path']}`)")
    lines.extend(["", "## Non-graphical checks", "", "| Check | Status |", "|---|---|"])
    lines.extend(
        f"| {check['check']}{'' if check['required'] else ' (informational)'} | {check['status']} |"
        for check in report["checks"]
    )
    lines.extend(["", "## Feature coverage", "", "| Feature | Status |", "|---|---|"])
    lines.extend(f"| {item['feature']} | {item['status']} |" for item in report["feature_coverage"])
    lines.extend(["", "## Graphics capability matrix", "", "Statuses are evidence-bounded: `unknown` is not treated as unsupported or as a zero-performance result.", "", "| Capability | Splinterm | Foot | Kitty | Ghostty | Alacritty |", "|---|---|---|---|---|---|"])
    for row in report["capability_matrix"]:
        statuses = row["statuses"]
        lines.append(f"| {row['capability']} | {statuses['splinterm']} | {statuses['foot']} | {statuses['kitty']} | {statuses['ghostty']} | {statuses['alacritty']} |")
    lines.extend(["", "## Portable external observations", "", "| Observation | Terminals | Cases | Claim boundary |", "|---|---:|---:|---|"])
    for item in report["external_observations"]:
        lines.append(f"| {item['observation']} | {len(item['terminals'])} | {item['measured_cases']} | {item['claim_limit']} |")
    lines.extend(["", "## Explicit limits", "", "- The checked-in parser fuzz target was not executed by this report and is not claimed as a fuzz pass.", "- Hyperlink handling and unsupported graphics protocols are not scored as failed performance runs.", "- Graphical Foot reference captures were not regenerated.", ""])
    return "\n".join(lines)


def write_report(output_dir: pathlib.Path, report: dict[str, Any]) -> None:
    output_dir.mkdir(parents=True, exist_ok=True)
    for name, content in (
        ("report.json", json.dumps(report, indent=2, sort_keys=True) + "\n"),
        ("README.md", render_markdown(report)),
    ):
        path = output_dir / name
        temporary = path.with_name(f".{name}.tmp")
        temporary.write_text(content, encoding="utf-8")
        temporary.replace(path)
