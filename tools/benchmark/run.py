#!/usr/bin/env python3
"""Portable, non-graphical Splinterbench commands."""

from __future__ import annotations

import argparse
import json
import pathlib
import sys
from typing import Any

from adapters import all_adapters
from manifest import collect
from latency import probe as probe_latency_boundary
from metrics import read_cgroup_v2, snapshot_process_tree
from summary import summarize_samples

ROOT = pathlib.Path(__file__).resolve().parents[2]
SCHEMA = pathlib.Path(__file__).with_name("result-schema.json")
LATENCY_SCHEMA = pathlib.Path(__file__).with_name("latency-schema.json")


def emit(value: Any, as_json: bool) -> None:
    if as_json:
        print(json.dumps(value, indent=2, sort_keys=True))


def command_probe(args: argparse.Namespace) -> int:
    identities = [adapter.probe(ROOT) for adapter in all_adapters()]
    if args.json:
        emit([identity.as_dict() for identity in identities], True)
    else:
        print("Terminal probes")
        for identity in identities:
            if not identity.available:
                print(f"  {identity.name:<10} unavailable")
                continue
            version = (identity.version or "version unknown").splitlines()[0]
            print(f"  {identity.name:<10} {version}")
        missing = [identity.name for identity in identities if not identity.available]
        if missing:
            print(f"\nUnavailable: {', '.join(missing)}")
        else:
            print("\nAll benchmark terminals are available.")
    return (
        1 if args.require_all and any(not item.available for item in identities) else 0
    )


def command_manifest(args: argparse.Namespace) -> int:
    value = collect(ROOT)
    args.output.parent.mkdir(parents=True, exist_ok=True)
    temporary = args.output.with_name(f".{args.output.name}.tmp")
    temporary.write_text(
        json.dumps(value, indent=2, sort_keys=True) + "\n", encoding="utf-8"
    )
    temporary.replace(args.output)
    print(f"Manifest written: {args.output}")
    available = sum(bool(item["available"]) for item in value["terminals"])
    print(f"Terminals found: {available}/{len(value['terminals'])}")
    return 0


def command_sample_process(args: argparse.Namespace) -> int:
    value = {"root_pid": args.pid, **snapshot_process_tree(args.pid).as_dict()}
    if args.json:
        emit(value, True)
    else:
        print("Process tree snapshot")
        print(f"  Processes        {value['process_count']}")
        print(f"  RSS              {value['rss_bytes']} bytes")
        print(f"  CPU              {value['cpu_ticks']} ticks")
        print(f"  Context switches {value['context_switches']}")
    return 0 if value["process_count"] else 1


def command_sample_cgroup(args: argparse.Namespace) -> int:
    value = {"path": str(args.path), **read_cgroup_v2(args.path)}
    if args.json:
        emit(value, True)
    else:
        print("Cgroup snapshot")
        for key, item in value.items():
            print(f"  {key:<22} {item if item is not None else 'unavailable'}")
    return 0 if args.path.is_dir() else 1


def command_summarize(args: argparse.Namespace) -> int:
    try:
        document = json.loads(args.result.read_text(encoding="utf-8"))
        summary = {
            "schema": "splinterm.benchmark.summary.v1",
            "source": str(args.result),
            "groups": summarize_samples(document["samples"]),
        }
    except (OSError, json.JSONDecodeError, KeyError, TypeError, ValueError) as error:
        print(f"Summary failed: {error}", file=sys.stderr)
        return 1
    text = json.dumps(summary, indent=2, sort_keys=True) + "\n"
    if args.output is None:
        sys.stdout.write(text)
    else:
        args.output.parent.mkdir(parents=True, exist_ok=True)
        args.output.write_text(text, encoding="utf-8")
        print(f"Summary written: {args.output}")
    return 0


def validate_document(result: pathlib.Path, schema_path: pathlib.Path) -> str | None:
    try:
        import jsonschema
    except ImportError:
        return "validation unavailable: install jsonschema"
    try:
        document = json.loads(result.read_text(encoding="utf-8"))
        schema = json.loads(schema_path.read_text(encoding="utf-8"))
        jsonschema.Draft202012Validator(
            schema, format_checker=jsonschema.FormatChecker()
        ).validate(document)
    except (OSError, json.JSONDecodeError, jsonschema.ValidationError) as error:
        return str(error)
    return None


def command_validate(args: argparse.Namespace) -> int:
    if error := validate_document(args.result, SCHEMA):
        print(f"Validation failed: {error}", file=sys.stderr)
        return 2 if error.startswith("validation unavailable") else 1
    print(f"Valid benchmark result: {args.result}")
    return 0


def command_probe_latency(args: argparse.Namespace) -> int:
    value = probe_latency_boundary()
    if args.json:
        emit(value, True)
    else:
        print("Targeted input latency boundary")
        print(f"  Backend       {value['backend']}")
        print(f"  Input         {value['input_protocol']}")
        print(f"  Capture       {value['capture_protocol']}")
        print(f"  Supported     {value['supported']}")
    return 0 if value["supported"] else 1


def command_validate_latency(args: argparse.Namespace) -> int:
    if error := validate_document(args.result, LATENCY_SCHEMA):
        print(f"Latency validation failed: {error}", file=sys.stderr)
        return 2 if error.startswith("validation unavailable") else 1
    print(f"Valid input-latency result: {args.result}")
    return 0


def parser() -> argparse.ArgumentParser:
    root = argparse.ArgumentParser(
        description="Inspect and prepare portable Splinterbench measurements; never launches windows"
    )
    commands = root.add_subparsers(dest="command", required=True)

    probe = commands.add_parser(
        "probe", help="inspect terminal availability and versions"
    )
    probe.add_argument(
        "--json", action="store_true", help="emit machine-readable output"
    )
    probe.add_argument(
        "--require-all", action="store_true", help="fail if any terminal is unavailable"
    )
    probe.set_defaults(handler=command_probe)

    manifest = commands.add_parser("manifest", help="write a reproducibility manifest")
    manifest.add_argument("output", type=pathlib.Path)
    manifest.set_defaults(handler=command_manifest)

    process = commands.add_parser(
        "sample-process", help="snapshot a Linux process tree"
    )
    process.add_argument("pid", type=int)
    process.add_argument(
        "--json", action="store_true", help="emit machine-readable output"
    )
    process.set_defaults(handler=command_sample_process)

    cgroup = commands.add_parser(
        "sample-cgroup", help="read an existing cgroup-v2 directory"
    )
    cgroup.add_argument("path", type=pathlib.Path)
    cgroup.add_argument(
        "--json", action="store_true", help="emit machine-readable output"
    )
    cgroup.set_defaults(handler=command_sample_cgroup)

    summarize = commands.add_parser(
        "summarize", help="summarize valid raw samples without hiding failures"
    )
    summarize.add_argument("result", type=pathlib.Path)
    summarize.add_argument("--output", type=pathlib.Path)
    summarize.set_defaults(handler=command_summarize)

    validate = commands.add_parser(
        "validate", help="validate a result against the checked-in schema"
    )
    validate.add_argument("result", type=pathlib.Path)
    validate.set_defaults(handler=command_validate)

    latency_probe = commands.add_parser(
        "probe-latency-boundary",
        help="probe targeted-input and screenshot dependencies without opening windows",
    )
    latency_probe.add_argument("--json", action="store_true")
    latency_probe.set_defaults(handler=command_probe_latency)

    validate_latency = commands.add_parser(
        "validate-latency", help="validate one targeted input-latency result"
    )
    validate_latency.add_argument("result", type=pathlib.Path)
    validate_latency.set_defaults(handler=command_validate_latency)
    return root


def main() -> int:
    args = parser().parse_args()
    return args.handler(args)


if __name__ == "__main__":
    raise SystemExit(main())
