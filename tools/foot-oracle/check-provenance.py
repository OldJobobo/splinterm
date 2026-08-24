#!/usr/bin/env python3
"""Validate repository-owned and pinned-host Foot oracle provenance."""

from __future__ import annotations

import argparse
import hashlib
import json
import os
import pathlib
import platform
import subprocess
import sys
from typing import Any

ROOT = pathlib.Path(__file__).resolve().parents[2]
MANIFEST = ROOT / "tools/foot-oracle/provenance.json"
UNSUPPORTED_EXIT = 77


class ProvenanceError(RuntimeError):
    """Pinned provenance is malformed or drifted."""


class UnsupportedHost(RuntimeError):
    """The current worker is not the declared raster reference host."""


def sha256(path: pathlib.Path) -> str:
    digest = hashlib.sha256()
    with path.open("rb") as source:
        for chunk in iter(lambda: source.read(1024 * 1024), b""):
            digest.update(chunk)
    return digest.hexdigest()


def load_manifest() -> dict[str, Any]:
    value = json.loads(MANIFEST.read_text(encoding="utf-8"))
    if value.get("schema") != 3:
        raise ProvenanceError("unsupported provenance schema")
    for key in (
        "supported_host",
        "reference",
        "build",
        "environment",
        "fonts",
        "rust",
        "oracle",
        "default_final_buffer_profile",
        "reference_update_policy",
    ):
        if key not in value:
            raise ProvenanceError(f"provenance is missing {key}")
    if len(value["fonts"]) != 6:
        raise ProvenanceError("provenance must declare all six raster faces")
    if value["reference_update_policy"].get("silent_regeneration") is not False:
        raise ProvenanceError("silent reference regeneration must remain disabled")
    return value


def check_repository_files(manifest: dict[str, Any]) -> None:
    expected_lock = manifest["rust"]["cargo_lock_sha256"]
    remediation = (
        "run 'python tools/foot-oracle/update-provenance.py' to review the change, "
        "then rerun with --write"
    )
    if sha256(ROOT / "Cargo.lock") != expected_lock:
        raise ProvenanceError(
            f"Cargo.lock drifted from pinned provenance; {remediation}"
        )
    profile_lock = manifest["default_final_buffer_profile"]["cargo_lock_sha256"]
    if profile_lock != expected_lock:
        raise ProvenanceError(
            f"profile and Rust Cargo.lock identities disagree; {remediation}"
        )
    for patch in manifest["oracle"]["patches"]:
        path = ROOT / patch["path"]
        if not path.is_file() or sha256(path) != patch["sha256"]:
            raise ProvenanceError(f"oracle patch drifted: {patch['path']}")


def os_release_id() -> str:
    values: dict[str, str] = {}
    for line in pathlib.Path("/etc/os-release").read_text(encoding="utf-8").splitlines():
        if "=" in line:
            key, value = line.split("=", 1)
            values[key] = value.strip().strip('"')
    return values.get("ID", "")


def require_supported_host(manifest: dict[str, Any]) -> None:
    expected = manifest["supported_host"]
    actual_os = os_release_id()
    actual_arch = platform.machine()
    if actual_os != expected["os_release_id"] or actual_arch != expected["architecture"]:
        raise UnsupportedHost(
            f"requires {expected['os_release_id']}/{expected['architecture']}; "
            f"worker is {actual_os or 'unknown'}/{actual_arch or 'unknown'}"
        )


def command_output(arguments: list[str]) -> str:
    result = subprocess.run(arguments, check=False, text=True, capture_output=True)
    if result.returncode != 0:
        raise ProvenanceError(f"command failed: {' '.join(arguments)}")
    return result.stdout.strip()


def active_fontconfig_fingerprint() -> str:
    home = str(pathlib.Path.home())
    entries = []
    for line in command_output(["fc-conflist"]).splitlines():
        if not line.startswith("+ "):
            continue
        path = pathlib.Path(line[2:].split(":", 1)[0])
        if not path.is_file():
            raise ProvenanceError(f"active fontconfig file is missing: {path}")
        name = str(path)
        if name.startswith(home):
            name = "~" + name[len(home) :]
        entries.append({"path": name, "sha256": sha256(path)})
    payload = json.dumps(entries, sort_keys=True, separators=(",", ":")).encode()
    return hashlib.sha256(payload).hexdigest()


def check_versions(manifest: dict[str, Any]) -> None:
    expected = manifest["build"]
    for package, key in (
        ("freetype2", "freetype_version"),
        ("fontconfig", "fontconfig_version"),
        ("pixman-1", "pixman_version"),
    ):
        if command_output(["pkg-config", "--modversion", package]) != expected[key]:
            raise ProvenanceError(f"{package} version drifted")
    rust = manifest["rust"]
    if command_output(["rustc", "--version"]).split()[1] != rust["rustc"]:
        raise ProvenanceError("rustc version drifted")
    if command_output(["cargo", "--version"]).split()[1] != rust["cargo"]:
        raise ProvenanceError("cargo version drifted")


def check_fonts(manifest: dict[str, Any]) -> None:
    for font in manifest["fonts"]:
        output = command_output(
            ["fc-match", "-f", "%{file}\n%{index}\n", font["pattern"]]
        ).splitlines()
        if len(output) < 2:
            raise ProvenanceError(f"fontconfig did not resolve {font['role']}")
        path = pathlib.Path(output[0])
        try:
            index = int(output[1])
        except ValueError as error:
            raise ProvenanceError(f"invalid face index for {font['role']}") from error
        if str(path) != font["file"] or index != font["index"]:
            raise ProvenanceError(f"resolved {font['role']} face drifted")
        if not path.is_file() or sha256(path) != font["sha256"]:
            raise ProvenanceError(f"resolved {font['role']} font bytes drifted")


def check_environment(manifest: dict[str, Any]) -> None:
    expected = manifest["environment"]
    if active_fontconfig_fingerprint() != expected["fontconfig_active_config_sha256"]:
        raise ProvenanceError("active fontconfig configuration drifted")
    for name, value in expected["variables"].items():
        actual = os.environ.get(name)
        if actual is not None and actual.startswith(str(pathlib.Path.home())):
            actual = "~" + actual[len(str(pathlib.Path.home())) :]
        if actual != value:
            raise ProvenanceError(f"environment variable drifted: {name}")


def main() -> int:
    parser = argparse.ArgumentParser()
    parser.add_argument("--portable", action="store_true")
    parser.add_argument("--ci-host", action="store_true")
    args = parser.parse_args()
    try:
        manifest = load_manifest()
        check_repository_files(manifest)
        if not args.portable:
            require_supported_host(manifest)
            check_versions(manifest)
            check_fonts(manifest)
            check_environment(manifest)
    except UnsupportedHost as error:
        if args.ci_host:
            print(f"UNSUPPORTED_ORACLE_HOST: {error}")
            return int(manifest.get("supported_host", {}).get("unsupported_exit_code", UNSUPPORTED_EXIT))
        print(f"provenance error: {error}", file=sys.stderr)
        return 1
    except (OSError, ValueError, KeyError, ProvenanceError) as error:
        print(f"provenance error: {error}", file=sys.stderr)
        return 1
    print("Foot oracle provenance: portable metadata valid" if args.portable else "Foot oracle provenance: pinned host exact")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
