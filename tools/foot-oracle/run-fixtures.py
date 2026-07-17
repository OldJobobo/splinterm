#!/usr/bin/env python3
"""Run semantic fixtures through the patched Foot reference executable."""

from __future__ import annotations

import difflib
import json
import os
import shlex
import subprocess
import sys
import tempfile
import time
from pathlib import Path
from typing import Any

ROOT = Path(__file__).resolve().parents[2]
FIXTURE_DIR = ROOT / "fixtures" / "terminal" / "v1"
DEFAULT_BINARY = Path("/tmp/splinterm-foot-oracle-build/foot")


def canonical(value: Any) -> str:
    return json.dumps(value, indent=2, ensure_ascii=False, sort_keys=True) + "\n"


def run_on_hyprland_workspace(
    command: list[str],
    environment: dict[str, str],
    workspace: int,
    fixture_id: str,
    output_dir: Path,
) -> tuple[int, str]:
    launcher = output_dir / f"launch-{fixture_id}.sh"
    done = output_dir / f"done-{fixture_id}"
    stdout = output_dir / f"stdout-{fixture_id}.log"
    stderr = output_dir / f"stderr-{fixture_id}.log"
    env_args = [
        "env",
        f"SPLINTERM_FOOT_STATE_DUMP={environment['SPLINTERM_FOOT_STATE_DUMP']}",
        f"SPLINTERM_FOOT_ORACLE_SIZE={environment['SPLINTERM_FOOT_ORACLE_SIZE']}",
    ]
    shell_command = shlex.join(env_args + command)
    launcher.write_text(
        "#!/usr/bin/env bash\n"
        "set +e\n"
        f"{shell_command} >{shlex.quote(str(stdout))} 2>{shlex.quote(str(stderr))}\n"
        "status=$?\n"
        f"printf '%s\\n' \"$status\" >{shlex.quote(str(done))}\n"
        "exit \"$status\"\n",
        encoding="utf-8",
    )
    launcher.chmod(0o700)

    lua_launcher = json.dumps(str(launcher))
    dispatched = subprocess.run(
        ["hyprctl", "eval", f"hl.exec_cmd({lua_launcher}, {{ workspace = {workspace} }})"],
        capture_output=True,
        text=True,
        timeout=5,
        check=False,
    )
    if dispatched.returncode != 0:
        return dispatched.returncode, dispatched.stderr or dispatched.stdout

    deadline = time.monotonic() + 10
    while not done.exists() and time.monotonic() < deadline:
        time.sleep(0.02)
    if not done.exists():
        return 124, "Hyprland-launched Foot oracle timed out"

    return int(done.read_text(encoding="utf-8").strip()), stderr.read_text(encoding="utf-8")


def workspace_clients(workspace: int) -> list[dict[str, Any]]:
    result = subprocess.run(
        ["hyprctl", "clients", "-j"],
        capture_output=True,
        text=True,
        timeout=5,
        check=False,
    )
    if result.returncode != 0:
        raise RuntimeError(result.stderr.strip() or "hyprctl clients failed")
    clients = json.loads(result.stdout)
    return [
        client
        for client in clients
        if client.get("workspace", {}).get("id") == workspace
    ]


def run_fixture(
    binary: Path,
    fixture_path: Path,
    output_dir: Path,
    hyprland_workspace: int | None,
) -> bool:
    fixture = json.loads(fixture_path.read_text(encoding="utf-8"))
    columns = fixture["initial"]["columns"]
    rows = fixture["initial"]["rows"]
    output_path = output_dir / f"{fixture['id']}.json"

    environment = os.environ.copy()
    environment["SPLINTERM_FOOT_STATE_DUMP"] = str(output_path)
    environment["SPLINTERM_FOOT_ORACLE_SIZE"] = f"{columns}x{rows}"

    child = (
        "import os,sys,time; "
        "os.write(1, bytes.fromhex(sys.argv[1])); "
        "time.sleep(0.05)"
    )
    command = [
        str(binary),
        "--config=/dev/null",
        "--override=pad=0x0",
        "--log-level=error",
        sys.executable,
        "-c",
        child,
        fixture["input_hex"],
    ]

    if hyprland_workspace is not None:
        returncode, stderr = run_on_hyprland_workspace(
            command,
            environment,
            hyprland_workspace,
            fixture["id"],
            output_dir,
        )
    else:
        try:
            result = subprocess.run(
                command,
                env=environment,
                capture_output=True,
                text=True,
                timeout=10,
                check=False,
            )
        except subprocess.TimeoutExpired:
            print(f"FAIL {fixture['id']}: Foot timed out", file=sys.stderr)
            return False
        returncode, stderr = result.returncode, result.stderr

    if returncode != 0:
        print(
            f"FAIL {fixture['id']}: Foot exited {returncode}\n{stderr}",
            file=sys.stderr,
        )
        return False
    if not output_path.exists():
        print(f"FAIL {fixture['id']}: Foot produced no state dump", file=sys.stderr)
        return False

    actual = json.loads(output_path.read_text(encoding="utf-8"))
    expected = fixture["expected"]
    if actual != expected:
        difference = difflib.unified_diff(
            canonical(expected).splitlines(keepends=True),
            canonical(actual).splitlines(keepends=True),
            fromfile=f"{fixture['id']}.expected",
            tofile=f"{fixture['id']}.foot",
        )
        print(f"FAIL {fixture['id']}", file=sys.stderr)
        print("".join(difference), file=sys.stderr)
        return False

    print(f"PASS {fixture['id']}")
    return True


def main() -> int:
    oracle_display = os.environ.get("FOOT_ORACLE_WAYLAND_DISPLAY")
    workspace_value = os.environ.get("FOOT_ORACLE_HYPRLAND_WORKSPACE")
    allow_live = os.environ.get("SPLINTERM_FOOT_ORACLE_ALLOW_LIVE_WAYLAND") == "1"
    hyprland_workspace = None

    if oracle_display:
        os.environ["WAYLAND_DISPLAY"] = oracle_display
    elif workspace_value:
        try:
            hyprland_workspace = int(workspace_value)
        except ValueError:
            print("FOOT_ORACLE_HYPRLAND_WORKSPACE must be a positive integer.", file=sys.stderr)
            return 2
        if hyprland_workspace <= 0:
            print("FOOT_ORACLE_HYPRLAND_WORKSPACE must be a positive integer.", file=sys.stderr)
            return 2
        if not os.environ.get("HYPRLAND_INSTANCE_SIGNATURE"):
            print("A Hyprland instance is required for workspace-routed oracle runs.", file=sys.stderr)
            return 2
        try:
            occupied = workspace_clients(hyprland_workspace)
        except (RuntimeError, json.JSONDecodeError) as error:
            print(f"Cannot inspect workspace {hyprland_workspace}: {error}", file=sys.stderr)
            return 2
        if occupied and os.environ.get("FOOT_ORACLE_ALLOW_OCCUPIED_WORKSPACE") != "1":
            print(
                f"Refusing to run: workspace {hyprland_workspace} contains "
                f"{len(occupied)} window(s).",
                file=sys.stderr,
            )
            return 2
    elif not allow_live:
        print(
            "Refusing to run the Foot oracle on the active workspace.\n"
            "Set FOOT_ORACLE_WAYLAND_DISPLAY to an isolated compositor socket, or\n"
            "set FOOT_ORACLE_HYPRLAND_WORKSPACE to an unused workspace.\n"
            "For deliberate interactive debugging only, set "
            "SPLINTERM_FOOT_ORACLE_ALLOW_LIVE_WAYLAND=1.",
            file=sys.stderr,
        )
        return 2
    elif not os.environ.get("WAYLAND_DISPLAY"):
        print("WAYLAND_DISPLAY is unset; the current Foot oracle requires a compositor.", file=sys.stderr)
        return 2

    binary = Path(os.environ.get("FOOT_ORACLE_BINARY", DEFAULT_BINARY))
    if not binary.is_file():
        print(
            f"Foot oracle binary not found: {binary}\n"
            "Run tools/foot-oracle/build-oracle.sh first.",
            file=sys.stderr,
        )
        return 2

    fixtures = sorted(FIXTURE_DIR.glob("*.json"))
    if not fixtures:
        print(f"No fixtures found in {FIXTURE_DIR}", file=sys.stderr)
        return 2

    with tempfile.TemporaryDirectory(prefix="splinterm-foot-oracle-") as directory:
        output_dir = Path(directory)
        passed = sum(
            run_fixture(binary, fixture, output_dir, hyprland_workspace)
            for fixture in fixtures
        )

    print(f"{passed}/{len(fixtures)} fixtures matched Foot.")
    return 0 if passed == len(fixtures) else 1


if __name__ == "__main__":
    raise SystemExit(main())
