#!/usr/bin/env python3
"""Build, capture, and compare the pinned Slice 1 final-buffer fixture matrix."""

from __future__ import annotations

import argparse
import hashlib
import json
import os
import re
import shlex
import shutil
import subprocess
import sys
import time
from pathlib import Path
from typing import Any

ROOT = Path(__file__).resolve().parents[2]
DEFAULT_BUILD = Path("/tmp/splinterm-foot-oracle-build")
DEFAULT_MANIFEST = ROOT / "tools/foot-oracle/final-buffer-fixtures.json"
APP_ID = "com.oldjobobo.splinterm.FinalBufferOracle"
TEST_WORKSPACE = 8
TEST_MONITOR = "DP-2"
CASE_ID = re.compile(r"^[A-Za-z0-9_.-]{1,128}$")


def run(command: list[str], **kwargs: Any) -> subprocess.CompletedProcess[str]:
    return subprocess.run(command, text=True, check=False, **kwargs)


def all_clients() -> list[dict[str, Any]]:
    result = run(["hyprctl", "clients", "-j"], capture_output=True, timeout=5)
    if result.returncode:
        raise RuntimeError(result.stderr.strip() or "hyprctl clients failed")
    return json.loads(result.stdout)


def workspace_clients(workspace: int) -> list[dict[str, Any]]:
    return [
        client
        for client in all_clients()
        if client.get("workspace", {}).get("id") == workspace
    ]


def hyprland_json(command: str) -> Any:
    result = run(["hyprctl", *shlex.split(command), "-j"], capture_output=True, timeout=5)
    if result.returncode:
        raise RuntimeError(result.stderr.strip() or f"hyprctl {command} failed")
    return json.loads(result.stdout)


def test_monitor_id() -> int:
    monitor = next(
        (monitor for monitor in hyprland_json("monitors all") if monitor.get("name") == TEST_MONITOR),
        None,
    )
    if monitor is None or monitor.get("disabled"):
        raise RuntimeError(f"required test monitor {TEST_MONITOR} is unavailable")
    if monitor.get("activeWorkspace", {}).get("id") != TEST_WORKSPACE:
        raise RuntimeError(
            f"{TEST_MONITOR} must already show reserved workspace {TEST_WORKSPACE}"
        )
    return int(monitor["id"])


def assert_test_workspace_isolated() -> int:
    monitor_id = test_monitor_id()
    workspace = next(
        (
            workspace
            for workspace in hyprland_json("workspaces")
            if workspace.get("id") == TEST_WORKSPACE
        ),
        None,
    )
    if workspace is None or workspace.get("monitor") != TEST_MONITOR:
        raise RuntimeError(
            f"workspace {TEST_WORKSPACE} is not assigned to {TEST_MONITOR}"
        )
    active = hyprland_json("activeworkspace")
    if active.get("id") == TEST_WORKSPACE or active.get("monitor") == TEST_MONITOR:
        raise RuntimeError(
            f"refusing to test on active {TEST_MONITOR}/workspace {TEST_WORKSPACE}"
        )
    if workspace_clients(TEST_WORKSPACE):
        raise RuntimeError(f"reserved workspace {TEST_WORKSPACE} is occupied")
    return monitor_id


def assert_user_workspace_untouched() -> None:
    active = hyprland_json("activeworkspace")
    if active.get("id") == TEST_WORKSPACE or active.get("monitor") == TEST_MONITOR:
        raise RuntimeError("oracle stole focus to the reserved test workspace")


def kill_oracle_window(address: str) -> None:
    selector = json.dumps(f"address:{address}")
    expression = f"hl.dispatch(hl.dsp.window.kill({{ window = {selector} }}))"
    run(["hyprctl", "eval", expression], capture_output=True, timeout=5)


def load_manifest(path: Path) -> dict[str, Any]:
    if path.stat().st_size > 1024 * 1024:
        raise ValueError("fixture manifest exceeds 1 MiB")
    manifest = json.loads(path.read_text(encoding="utf-8"))
    if manifest.get("schema") != "splinterm.final-buffer-fixtures.v1":
        raise ValueError("unsupported fixture manifest schema")
    profile = manifest.get("profile")
    cases = manifest.get("cases")
    if not isinstance(profile, dict) or not isinstance(cases, list) or not 1 <= len(cases) <= 64:
        raise ValueError("fixture manifest profile/cases are invalid")
    if profile.get("scale_120") != 120 or profile.get("padding") != 12:
        raise ValueError("Slice 1 supports only the pinned 1x/12 px symmetric-padding profile")
    if not isinstance(profile.get("font"), str) or not 6.0 <= profile.get("font_size", 0) <= 96.0:
        raise ValueError("fixture font profile is invalid")
    seen: set[str] = set()
    for case in cases:
        if not isinstance(case, dict) or not CASE_ID.fullmatch(str(case.get("id", ""))):
            raise ValueError("fixture ID is invalid")
        if case["id"] in seen:
            raise ValueError(f"duplicate fixture ID: {case['id']}")
        seen.add(case["id"])
        columns, rows, text = case.get("columns"), case.get("rows"), case.get("text")
        if (
            isinstance(columns, bool)
            or not isinstance(columns, int)
            or not 1 <= columns <= 4096
            or isinstance(rows, bool)
            or not isinstance(rows, int)
            or not 1 <= rows <= 4096
            or not isinstance(text, str)
            or len(text.encode("utf-8")) > 1024 * 1024
            or len(text) > columns * rows
        ):
            raise ValueError(f"fixture {case['id']} grid/text is invalid")
        if case.get("style") not in ("normal", "reverse", "dim", "conceal"):
            raise ValueError(f"fixture {case['id']} style is invalid")
        cursor = case.get("cursor")
        if not isinstance(cursor, dict) or not isinstance(cursor.get("visible"), bool):
            raise ValueError(f"fixture {case['id']} cursor is invalid")
        if cursor["visible"]:
            if (
                cursor.get("shape") not in ("block", "beam", "underline")
                or not isinstance(cursor.get("column"), int)
                or not 0 <= cursor["column"] < columns
                or not isinstance(cursor.get("row"), int)
                or not 0 <= cursor["row"] < rows
            ):
                raise ValueError(f"fixture {case['id']} cursor is out of bounds")
    return manifest


def sha256(path: Path) -> str:
    digest = hashlib.sha256()
    with path.open("rb") as source:
        for block in iter(lambda: source.read(1024 * 1024), b""):
            digest.update(block)
    return digest.hexdigest()


def preflight_provenance(manifest: dict[str, Any]) -> dict[str, Any]:
    provenance_path = ROOT / "tools/foot-oracle/provenance.json"
    provenance = json.loads(provenance_path.read_text(encoding="utf-8"))
    if provenance.get("schema") != 3:
        raise ValueError("unsupported oracle provenance schema")
    source = Path(os.environ.get("FOOT_SOURCE", Path.home() / "Playground/foot"))
    revision = run(["git", "-C", str(source), "rev-parse", "HEAD"], capture_output=True)
    expected_revision = provenance["reference"]["commit"]
    if revision.returncode or revision.stdout.strip() != expected_revision:
        raise ValueError("Foot source revision drifted from pinned provenance")
    for patch in provenance["oracle"]["patches"]:
        path = ROOT / patch["path"]
        if sha256(path) != patch["sha256"]:
            raise ValueError(f"oracle patch hash drift: {patch['path']}")
    profile = provenance["default_final_buffer_profile"]
    if sha256(ROOT / "Cargo.lock") != profile["cargo_lock_sha256"]:
        raise ValueError("Cargo.lock drifted from final-buffer provenance")
    font_match = run(
        [
            "fc-match",
            "-f",
            "%{file}\\n%{index}\\n",
            profile["font_pattern"],
        ],
        capture_output=True,
    )
    if font_match.returncode:
        raise ValueError("fc-match failed during provenance preflight")
    lines = font_match.stdout.splitlines()
    if (
        len(lines) < 2
        or lines[0] != profile["font_file"]
        or int(lines[1]) != profile["font_index"]
        or sha256(Path(lines[0])) != profile["font_sha256"]
    ):
        raise ValueError("resolved primary font drifted from pinned provenance")
    for package, key in (
        ("freetype2", "freetype_version"),
        ("fontconfig", "fontconfig_version"),
        ("pixman-1", "pixman_version"),
    ):
        version = run(["pkg-config", "--modversion", package], capture_output=True)
        if version.returncode or version.stdout.strip() != provenance["build"][key]:
            raise ValueError(f"{package} version drifted from pinned provenance")
    expected_profile = {
        "font": manifest["profile"]["font"],
        "font_size": manifest["profile"]["font_size"],
        "scale_120": manifest["profile"]["scale_120"],
        "padding": manifest["profile"]["padding"],
        "foreground": manifest["profile"]["foreground"],
        "background": manifest["profile"]["background"],
    }
    if expected_profile != {
        "font": profile["font_pattern"].removesuffix(":pixelsize=12"),
        "font_size": profile["logical_size_px"],
        "scale_120": profile["scale_120"],
        "padding": profile["padding"]["left"],
        "foreground": profile["foreground"],
        "background": profile["background"],
    }:
        raise ValueError("fixture profile drifted from pinned provenance")
    return provenance


def foot_payload(case: dict[str, Any]) -> bytes:
    style = {"normal": 0, "reverse": 7, "dim": 2, "conceal": 8}[case["style"]]
    payload = bytearray(b"\x1b[?25l")
    if style:
        payload.extend(f"\x1b[{style}m".encode())
    payload.extend(case["text"].encode("utf-8"))
    cursor = case["cursor"]
    if cursor["visible"]:
        shape = {"block": 2, "underline": 4, "beam": 6}[cursor["shape"]]
        payload.extend(f"\x1b[{cursor['row'] + 1};{cursor['column'] + 1}H".encode())
        payload.extend(f"\x1b[{shape} q\x1b[?25h".encode())
    return bytes(payload)


def capture_splinterm(
    binary: Path,
    output_prefix: Path,
    profile: dict[str, Any],
    case: dict[str, Any],
    logical_size: tuple[int, int] | None = None,
) -> dict[str, Any]:
    cursor = case["cursor"]
    command = [
        str(binary),
        "--output-prefix",
        str(output_prefix),
        "--fixture",
        case["id"],
        "--frame-id",
        case["id"],
        "--text-hex",
        case["text"].encode("utf-8").hex(),
        "--style",
        case["style"],
        "--font",
        profile["font"],
        "--font-size",
        str(profile["font_size"]),
        "--scale-120",
        str(profile["scale_120"]),
        "--columns",
        str(case["columns"]),
        "--rows",
        str(case["rows"]),
        "--cursor-shape",
        cursor.get("shape", "block"),
        "--cursor-column",
        str(cursor.get("column", 0)),
        "--cursor-row",
        str(cursor.get("row", 0)),
    ]
    if not cursor["visible"]:
        command.append("--hide-cursor")
    if logical_size is not None:
        command.extend(
            [
                "--logical-width",
                str(logical_size[0]),
                "--logical-height",
                str(logical_size[1]),
            ]
        )
    result = run(command, cwd=ROOT, capture_output=True)
    if result.returncode:
        raise RuntimeError(result.stderr.strip() or "Splinterm capture failed")
    return json.loads(output_prefix.with_suffix(".json").read_text(encoding="utf-8"))


def wait_for_oracle_windows_to_close(timeout: float = 3.0) -> None:
    deadline = time.monotonic() + timeout
    while time.monotonic() < deadline:
        if not any(client.get("class") == APP_ID for client in all_clients()):
            return
        time.sleep(0.02)
    raise RuntimeError("previous Foot oracle window did not close")


def capture_foot(
    binary: Path,
    output_prefix: Path,
    output_dir: Path,
    profile: dict[str, Any],
    provenance: dict[str, Any],
    case: dict[str, Any],
    workspace: int,
    monitor_id: int,
    width: int,
    height: int,
    *,
    payload_override: bytes | None = None,
    extra_environment: list[str] | None = None,
    extra_overrides: list[str] | None = None,
) -> dict[str, Any]:
    wait_for_oracle_windows_to_close()
    assert_user_workspace_untouched()
    if workspace_clients(workspace):
        raise RuntimeError(f"workspace {workspace} became occupied")
    existing_addresses = {client.get("address") for client in all_clients()}
    payload = payload_override if payload_override is not None else foot_payload(case)
    child = (
        "import os,sys,time; time.sleep(2); "
        "open(os.environ['FOOT_ORACLE_BUFFER_PREFIX']+'.capture','w').close(); "
        "os.write(1,bytes.fromhex(sys.argv[1])); time.sleep(2)"
    )
    font_family = profile["font"].split(":", 1)[0]
    command = [
        "env",
        f"FOOT_ORACLE_BUFFER_PREFIX={output_prefix}",
        f"FOOT_ORACLE_FIXTURE={case['id']}",
        f"FOOT_ORACLE_FRAME_ID={case['id']}",
        f"FOOT_ORACLE_FONT_FILE={provenance['default_final_buffer_profile']['font_file']}",
        f"FOOT_ORACLE_FONT_SHA256={provenance['default_final_buffer_profile']['font_sha256']}",
        f"FOOT_ORACLE_FCFT_VERSION={provenance['build']['fcft_version']}",
        f"SPLINTERM_FOOT_ORACLE_SIZE={case['columns']}x{case['rows']}",
        *(extra_environment or []),
        str(binary),
        "--config=/dev/null",
        f"--override=pad={profile['padding']}x{profile['padding']}",
        f"--override=colors.background={profile['background']}",
        f"--override=colors.foreground={profile['foreground']}",
        "--override=cursor.unfocused-style=unchanged",
        *(extra_overrides or []),
        f"--font={font_family}:pixelsize={profile['font_size']:g}",
        f"--window-size-chars={case['columns']}x{case['rows']}",
        f"--app-id={APP_ID}",
        sys.executable,
        "-c",
        child,
        payload.hex(),
    ]
    launcher = output_dir / "launch-foot.sh"
    launcher.write_text(
        "#!/usr/bin/env bash\n"
        + "exec "
        + shlex.join(command)
        + f" >{shlex.quote(str(output_dir / 'foot.stdout'))} 2>{shlex.quote(str(output_dir / 'foot.stderr'))}\n",
        encoding="utf-8",
    )
    launcher.chmod(0o700)
    expression = (
        f"hl.exec_cmd({json.dumps(str(launcher))}, "
        f"{{ workspace = '{workspace} silent', float = true, size = '{width} {height}', no_initial_focus = true }})"
    )
    dispatched = run(["hyprctl", "eval", expression], capture_output=True, timeout=5)
    if dispatched.returncode:
        raise RuntimeError(dispatched.stderr or dispatched.stdout)

    oracle_client = None
    deadline = time.monotonic() + 5
    while oracle_client is None and time.monotonic() < deadline:
        oracle_client = next(
            (
                client
                for client in all_clients()
                if client.get("class") == APP_ID
                and client.get("address") not in existing_addresses
            ),
            None,
        )
        if oracle_client is None:
            time.sleep(0.01)
    if oracle_client is None:
        raise RuntimeError("Foot oracle window did not map")
    address = oracle_client["address"]
    try:
        if (
            oracle_client.get("workspace", {}).get("id") != workspace
            or oracle_client.get("monitor") != monitor_id
        ):
            actual_workspace = oracle_client.get("workspace", {}).get("id")
            actual_monitor = oracle_client.get("monitor")
            raise RuntimeError(
                f"Foot oracle escaped workspace {workspace} on {TEST_MONITOR}: "
                f"mapped to workspace {actual_workspace} on monitor {actual_monitor}"
            )
        assert_user_workspace_untouched()
        selector = json.dumps(f"address:{address}")
        if not oracle_client.get("floating"):
            expression = (
                "hl.dispatch(hl.dsp.window.float("
                f"{{ action = 'enable', window = {selector} }}))"
            )
            result = run(["hyprctl", "eval", expression], capture_output=True, timeout=5)
            if result.returncode:
                raise RuntimeError(result.stderr or result.stdout)
            time.sleep(0.05)
        resize = (
            "hl.dispatch(hl.dsp.window.resize("
            f"{{ x = {width}, y = {height}, window = {selector} }}))"
        )
        deadline = time.monotonic() + 2.0
        while time.monotonic() < deadline:
            result = run(["hyprctl", "eval", resize], capture_output=True, timeout=5)
            if result.returncode:
                raise RuntimeError(result.stderr or result.stdout)
            time.sleep(0.03)
            current = next(
                (client for client in all_clients() if client.get("address") == address),
                None,
            )
            if current is not None and (
                current.get("workspace", {}).get("id") != workspace
                or current.get("monitor") != monitor_id
            ):
                raise RuntimeError("Foot oracle moved outside the reserved test workspace")
            assert_user_workspace_untouched()
            if output_prefix.with_suffix(".json").exists():
                break

        deadline = time.monotonic() + 10
        while not output_prefix.with_suffix(".json").exists() and time.monotonic() < deadline:
            time.sleep(0.02)
        if not output_prefix.with_suffix(".json").exists():
            stderr = output_dir / "foot.stderr"
            raise RuntimeError(
                stderr.read_text() if stderr.exists() else "Foot produced no final-buffer capture"
            )
        metadata = json.loads(output_prefix.with_suffix(".json").read_text(encoding="utf-8"))
        current = next(
            (client for client in all_clients() if client.get("address") == address),
            None,
        )
        metadata["_oracle_logical_size"] = (
            current.get("size") if current is not None else [width, height]
        )
        wait_for_oracle_windows_to_close()
        assert_user_workspace_untouched()
        return metadata
    finally:
        try:
            still_mapped = any(
                client.get("address") == address for client in all_clients()
            )
        except (OSError, RuntimeError, json.JSONDecodeError):
            still_mapped = True
        if still_mapped:
            kill_oracle_window(address)


def read_comparison_result(
    comparison: subprocess.CompletedProcess[str], report_path: Path
) -> dict[str, Any]:
    if comparison.returncode not in (0, 1):
        raise RuntimeError(comparison.stderr.strip() or "comparison failed")
    if not report_path.exists():
        raise RuntimeError("comparator produced no report")
    return json.loads(report_path.read_text(encoding="utf-8"))


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("output_dir", type=Path)
    parser.add_argument("--workspace", type=int, default=TEST_WORKSPACE)
    parser.add_argument("--manifest", type=Path, default=DEFAULT_MANIFEST)
    args = parser.parse_args()
    if args.workspace != TEST_WORKSPACE or not os.environ.get("HYPRLAND_INSTANCE_SIGNATURE"):
        parser.error(f"tests are restricted to workspace {TEST_WORKSPACE} on {TEST_MONITOR}")
    try:
        manifest = load_manifest(args.manifest)
        provenance = preflight_provenance(manifest)
        monitor_id = assert_test_workspace_isolated()
        occupied = workspace_clients(args.workspace)
    except (OSError, ValueError, RuntimeError, json.JSONDecodeError) as error:
        parser.error(str(error))
    if occupied:
        parser.error(f"workspace {args.workspace} contains {len(occupied)} window(s)")

    build_dir = Path(os.environ.get("FOOT_ORACLE_BUILD", DEFAULT_BUILD))
    foot_binary = build_dir / "foot"
    result = run([str(ROOT / "tools/foot-oracle/build-oracle.sh")], cwd=ROOT)
    if result.returncode:
        return result.returncode
    if not foot_binary.is_file():
        parser.error(f"patched Foot binary is missing: {foot_binary}")
    result = run(
        ["cargo", "build", "-q", "-p", "splinterm", "--bin", "final-buffer-capture"],
        cwd=ROOT,
    )
    if result.returncode:
        return result.returncode
    splinterm_binary = ROOT / "target/debug/final-buffer-capture"

    args.output_dir.mkdir(parents=True, exist_ok=True)
    summary: dict[str, Any] = {
        "schema": "splinterm.final-buffer-matrix.v1",
        "manifest": str(args.manifest),
        "provenance_schema": provenance["schema"],
        "foot_commit": provenance["reference"]["commit"],
        "cases": [],
        "exact": False,
    }
    for case in manifest["cases"]:
        case_dir = args.output_dir / case["id"]
        case_dir.mkdir(parents=True, exist_ok=True)
        shutil.rmtree(case_dir / "diff", ignore_errors=True)
        foot_prefix = case_dir / "foot"
        splinterm_prefix = case_dir / "splinterm"
        for prefix in (foot_prefix, splinterm_prefix):
            for suffix in (".argb", ".json", ".capture"):
                prefix.with_suffix(suffix).unlink(missing_ok=True)
        for transient in ("foot.done", "foot.stdout", "foot.stderr"):
            (case_dir / transient).unlink(missing_ok=True)
        record: dict[str, Any] = {"id": case["id"], "exact": False}
        try:
            metadata = capture_splinterm(splinterm_binary, splinterm_prefix, manifest["profile"], case)
            foot_metadata = capture_foot(
                foot_binary,
                foot_prefix,
                case_dir,
                manifest["profile"],
                provenance,
                case,
                args.workspace,
                monitor_id,
                metadata["width"],
                metadata["height"],
            )
            foot_size = (int(foot_metadata["width"]), int(foot_metadata["height"]))
            if foot_size != (metadata["width"], metadata["height"]):
                metadata = capture_splinterm(
                    splinterm_binary,
                    splinterm_prefix,
                    manifest["profile"],
                    case,
                    foot_size,
                )
            comparison = run(
                [
                    sys.executable,
                    str(ROOT / "tools/foot-oracle/compare-final-buffers.py"),
                    "--reference-metadata",
                    str(foot_prefix.with_suffix(".json")),
                    "--actual-metadata",
                    str(splinterm_prefix.with_suffix(".json")),
                    "--output-dir",
                    str(case_dir / "diff"),
                ],
                cwd=ROOT,
                capture_output=True,
            )
            report = read_comparison_result(
                comparison, case_dir / "diff/comparison.json"
            )
            record.update(
                exact=bool(report["exact"]),
                mismatch_pixels=report["mismatch_pixels"],
                maximum_channel_delta=report["maximum_channel_delta"],
            )
        except (OSError, RuntimeError, ValueError, json.JSONDecodeError) as error:
            record["exact"] = False
            record["error"] = str(error)
        summary["cases"].append(record)
        print(f"{'PASS' if record['exact'] else 'FAIL'} {case['id']}")

    summary["exact"] = all(case["exact"] for case in summary["cases"])
    (args.output_dir / "summary.json").write_text(
        json.dumps(summary, indent=2, sort_keys=True) + "\n", encoding="utf-8"
    )
    print(f"Final-buffer matrix: {sum(case['exact'] for case in summary['cases'])}/{len(summary['cases'])} exact")
    return 0 if summary["exact"] else 1


if __name__ == "__main__":
    raise SystemExit(main())
