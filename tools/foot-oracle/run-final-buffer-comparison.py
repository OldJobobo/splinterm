#!/usr/bin/env python3
"""Build, capture, and compare the default Foot/Splinterm final buffer."""

from __future__ import annotations

import argparse
import json
import os
import shlex
import subprocess
import sys
import time
from pathlib import Path

ROOT = Path(__file__).resolve().parents[2]
DEFAULT_FOOT = Path("/tmp/splinterm-foot-oracle-build/foot")


def run(command, **kwargs):
    return subprocess.run(command, text=True, check=False, **kwargs)


def all_clients():
    result = run(["hyprctl", "clients", "-j"], capture_output=True, timeout=5)
    if result.returncode:
        raise RuntimeError(result.stderr.strip() or "hyprctl clients failed")
    return json.loads(result.stdout)


def workspace_clients(workspace):
    return [
        client
        for client in all_clients()
        if client.get("workspace", {}).get("id") == workspace
    ]


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("output_dir", type=Path)
    parser.add_argument("--workspace", type=int, default=8)
    parser.add_argument("--skip-build", action="store_true")
    args = parser.parse_args()
    if args.workspace <= 0 or not os.environ.get("HYPRLAND_INSTANCE_SIGNATURE"):
        parser.error("a positive empty Hyprland workspace is required")
    try:
        occupied = workspace_clients(args.workspace)
    except (RuntimeError, json.JSONDecodeError) as error:
        parser.error(str(error))
    if occupied:
        parser.error(f"workspace {args.workspace} contains {len(occupied)} window(s)")
    if not args.skip_build:
        result = run([str(ROOT / "tools/foot-oracle/build-oracle.sh")], cwd=ROOT)
        if result.returncode:
            return result.returncode
    if not DEFAULT_FOOT.is_file():
        parser.error(f"patched Foot binary is missing: {DEFAULT_FOOT}")

    args.output_dir.mkdir(parents=True, exist_ok=True)
    foot_prefix = args.output_dir / "foot-ascii"
    splinterm_prefix = args.output_dir / "splinterm-ascii"
    for prefix in (foot_prefix, splinterm_prefix):
        prefix.with_suffix(".argb").unlink(missing_ok=True)
        prefix.with_suffix(".json").unlink(missing_ok=True)
        prefix.with_suffix(".capture").unlink(missing_ok=True)

    corpus = "".join(chr(value) for value in range(0x20, 0x7F))
    child = "import os,sys,time; time.sleep(2); open(os.environ['FOOT_ORACLE_BUFFER_PREFIX']+'.capture','w').close(); os.write(1,b'\\x1b[?25l'+sys.argv[1].encode('ascii')); time.sleep(4)"
    command = [
        "env",
        f"FOOT_ORACLE_BUFFER_PREFIX={foot_prefix}",
        "FOOT_ORACLE_FIXTURE=ascii",
        "FOOT_ORACLE_FRAME_ID=ascii-regular-12px-1x",
        "SPLINTERM_FOOT_ORACLE_SIZE=95x1",
        str(DEFAULT_FOOT),
        "--config=/dev/null",
        "--override=pad=12x12",
        "--override=colors.background=0e1216",
        "--override=colors.foreground=ebebeb",
        "--font=JetBrains Mono Nerd Font:pixelsize=12",
        "--window-size-chars=95x1",
        "--app-id=com.oldjobobo.splinterm.FinalBufferOracle",
        sys.executable,
        "-c",
        child,
        corpus,
    ]
    done = args.output_dir / "foot.done"
    launcher = args.output_dir / "launch-foot.sh"
    launcher.write_text(
        "#!/usr/bin/env bash\nset +e\n"
        + shlex.join(command)
        + f" >{shlex.quote(str(args.output_dir / 'foot.stdout'))} 2>{shlex.quote(str(args.output_dir / 'foot.stderr'))}\n"
        + "status=$?\n"
        + f"printf '%s\\n' \"$status\" >{shlex.quote(str(done))}\n",
        encoding="utf-8",
    )
    launcher.chmod(0o700)
    expression = (
        f"hl.exec_cmd({json.dumps(str(launcher))}, "
        f"{{ workspace = {args.workspace}, float = true, size = '689 41' }})"
    )
    dispatched = run(
        ["hyprctl", "eval", expression], capture_output=True, timeout=5
    )
    if dispatched.returncode:
        print(dispatched.stderr or dispatched.stdout, file=sys.stderr)
        return dispatched.returncode
    oracle_client = None
    map_deadline = time.monotonic() + 5
    while oracle_client is None and time.monotonic() < map_deadline:
        oracle_client = next(
            (
                client
                for client in all_clients()
                if client.get("class") == "com.oldjobobo.splinterm.FinalBufferOracle"
            ),
            None,
        )
        if oracle_client is None:
            time.sleep(0.01)
    if oracle_client is None:
        print("Foot oracle window did not map", file=sys.stderr)
        return 1
    address = oracle_client["address"]
    if not oracle_client.get("floating"):
        float_window = f"hl.dispatch(hl.dsp.window.float('address:{address}'))"
        result = run(["hyprctl", "eval", float_window], capture_output=True, timeout=5)
        if result.returncode:
            print(result.stderr or result.stdout, file=sys.stderr)
            return result.returncode
        time.sleep(0.05)
    resize_window = f"hl.dispatch(hl.dsp.window.resize('exact 689 41,address:{address}'))"
    configured = False
    configure_deadline = time.monotonic() + 1.5
    while not configured and time.monotonic() < configure_deadline:
        result = run(["hyprctl", "eval", resize_window], capture_output=True, timeout=5)
        if result.returncode:
            print(result.stderr or result.stdout, file=sys.stderr)
            return result.returncode
        time.sleep(0.03)
        current = next((client for client in all_clients() if client.get("address") == address), None)
        configured = bool(
            current
            and current.get("floating")
            and current.get("size") == [689, 41]
        )
    if not configured:
        print("Foot oracle window did not reach the declared floating size", file=sys.stderr)
        return 1
    deadline = time.monotonic() + 15
    while not done.exists() and time.monotonic() < deadline:
        time.sleep(0.02)
    if not done.exists() or done.read_text().strip() != "0":
        stderr = args.output_dir / "foot.stderr"
        print(stderr.read_text() if stderr.exists() else "Foot launcher did not complete", file=sys.stderr)
        return 1
    if not foot_prefix.with_suffix(".json").exists():
        print("Foot produced no final-buffer capture", file=sys.stderr)
        return 1

    result = run(
        [
            "cargo", "run", "-q", "-p", "splinterm", "--bin", "final-buffer-capture", "--",
            "--output-prefix", str(splinterm_prefix), "--fixture", "ascii", "--font-size", "12",
            "--scale-120", "120", "--columns", "95", "--rows", "1", "--hide-cursor",
            "--frame-id", "ascii-regular-12px-1x",
        ],
        cwd=ROOT,
    )
    if result.returncode:
        return result.returncode
    return run(
        [
            sys.executable, str(ROOT / "tools/foot-oracle/compare-final-buffers.py"),
            "--reference-metadata", str(foot_prefix.with_suffix(".json")),
            "--actual-metadata", str(splinterm_prefix.with_suffix(".json")),
            "--output-dir", str(args.output_dir / "diff"),
        ],
        cwd=ROOT,
    ).returncode


if __name__ == "__main__":
    raise SystemExit(main())
