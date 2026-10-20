#!/usr/bin/env python3
"""Splinterm MCP demo harness (Plan 0031).

Milestone 1 implements only the non-graphical `preflight` lane:

  run-demo.py preflight --staged-bin-dir DIR [--evidence-dir DIR]
  run-demo.py preflight --installed [--bin-dir /usr/bin] [--evidence-dir DIR]
      (installed mode: not yet validated against a real installed package)

The preflight performs no Wayland, compositor, or window action, writes no
policy, and leaves no daemon, socket, process, or temporary directory behind.
The graphical lane is not implemented yet and requires its own approval.

Exit status: 0 passed, 1 preflight_failed, 2 cleanup_incomplete, 3 aborted,
64 usage error.
"""

from __future__ import annotations

import argparse
import json
from pathlib import Path
import signal
import sys

sys.path.insert(0, str(Path(__file__).resolve().parent))

from mcp_preflight import (  # noqa: E402
    CLEANUP_SIGNALS,
    DEFAULT_SCHEMA_ROOT,
    REPO_ROOT,
    Options,
    Preflight,
    PreflightAbort,
    interrupt_once,
    select_binaries,
)

EXIT_CODES = {"passed": 0, "preflight_failed": 1, "cleanup_incomplete": 2, "aborted": 3}


def parser() -> argparse.ArgumentParser:
    result = argparse.ArgumentParser(description=__doc__.splitlines()[0])
    commands = result.add_subparsers(dest="command", required=True)
    preflight = commands.add_parser("preflight", help="non-graphical identity, protocol, and deny-all preflight")
    mode = preflight.add_mutually_exclusive_group(required=True)
    mode.add_argument("--installed", action="store_true", help="use the installed adjacent set (default /usr/bin; not yet validated against a real package)")
    mode.add_argument("--staged-bin-dir", type=Path, help="directory holding sibling staged binaries")
    preflight.add_argument("--bin-dir", type=Path, default=Path("/usr/bin"), help="installed binary directory")
    preflight.add_argument("--evidence-dir", type=Path, help="existing directory that receives mcp-demo-<UTC>-<nonce>/")
    preflight.add_argument("--schema-root", type=Path, default=DEFAULT_SCHEMA_ROOT, help="MCP v2 schema root")
    preflight.add_argument("--source-root", type=Path, default=REPO_ROOT, help="source tree recorded for staged mode")
    return result


def main(argv: list[str] | None = None) -> int:
    try:
        arguments = parser().parse_args(argv)
    except SystemExit as exit_request:
        return 0 if exit_request.code == 0 else 64
    if arguments.installed:
        mode, bin_dir = "installed", arguments.bin_dir
    else:
        mode, bin_dir = "staged", arguments.staged_bin_dir
    try:
        selection = select_binaries(mode, bin_dir)
    except PreflightAbort as abort:
        print(json.dumps({"result": "preflight_failed", "error": abort.code}), flush=True)
        return EXIT_CODES["preflight_failed"]
    # One-shot: the first SIGINT/SIGTERM/SIGHUP unwinds into ordered cleanup
    # and every later one is ignored, so a repeated signal cannot skip it.
    for signum in CLEANUP_SIGNALS:
        signal.signal(signum, interrupt_once)
    options = Options(
        evidence_base=arguments.evidence_dir,
        schema_root=arguments.schema_root,
        source_root=arguments.source_root,
    )
    try:
        runner = Preflight(selection, options)
    except (OSError, ValueError) as error:
        print(json.dumps({"result": "preflight_failed", "error": "evidence_dir_unusable", "detail": type(error).__name__}), flush=True)
        return EXIT_CODES["preflight_failed"]
    report = runner.run()
    summary = report.one_line()
    if runner.evidence.directory is not None:
        summary["evidence"] = str(runner.evidence.directory)
    summary["evidence_complete"] = runner.evidence_error is None
    if runner.evidence_error is not None:
        summary["evidence_error"] = runner.evidence_error
    print(json.dumps(summary, sort_keys=True), flush=True)
    return EXIT_CODES.get(report.result, 1)


if __name__ == "__main__":
    sys.exit(main())
