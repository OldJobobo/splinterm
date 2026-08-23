#!/usr/bin/env python3
"""Assemble a non-publishing Splinterm release candidate from validated packages."""

from __future__ import annotations

import argparse
import hashlib
import json
from pathlib import Path
import re
import shutil
import subprocess
import sys
from typing import Any

ROOT = Path(__file__).resolve().parents[2]
SEMVER = re.compile(r"[0-9]+\.[0-9]+\.[0-9]+(?:-[0-9A-Za-z.-]+)?")
COMMIT = re.compile(r"[0-9a-f]{40}")
REPOSITORY = re.compile(r"[A-Za-z0-9_.-]+/[A-Za-z0-9_.-]+")
SHA256 = re.compile(r"[0-9a-f]{64}")
SCHEMA = 1


def run(arguments: list[str], *, cwd: Path = ROOT) -> str:
    result = subprocess.run(
        arguments,
        cwd=cwd,
        text=True,
        capture_output=True,
        check=False,
    )
    if result.returncode != 0:
        detail = result.stderr.strip() or result.stdout.strip()
        raise ValueError(f"command failed ({' '.join(arguments)}): {detail}")
    return result.stdout.strip()


def sha256(path: Path) -> str:
    digest = hashlib.sha256()
    with path.open("rb") as stream:
        for chunk in iter(lambda: stream.read(1024 * 1024), b""):
            digest.update(chunk)
    return digest.hexdigest()


def assignment(path: Path, name: str) -> str:
    pattern = re.compile(rf"^{re.escape(name)}=([^\n]+)$", re.MULTILINE)
    match = pattern.search(path.read_text(encoding="utf-8"))
    if match is None:
        raise ValueError(f"{path.relative_to(ROOT)} does not define {name}")
    return match.group(1).strip("'\"")


def workspace_version() -> str:
    text = (ROOT / "Cargo.toml").read_text(encoding="utf-8")
    section = text.split("[workspace.package]", 1)
    if len(section) != 2:
        raise ValueError("Cargo.toml has no [workspace.package] section")
    match = re.search(r'^version\s*=\s*"([^"]+)"$', section[1], re.MULTILINE)
    if match is None:
        raise ValueError("workspace package version is unavailable")
    return match.group(1)


def arch_version(version: str) -> str:
    return version.replace("-", "")


def package_layout(path: Path) -> list[str]:
    lines = path.read_text(encoding="utf-8").splitlines()
    return [
        line.rstrip()
        for line in lines
        if line.startswith("  install ") or line.startswith("  ln -s ")
    ]


def validate_versions(version: str) -> str:
    if SEMVER.fullmatch(version) is None:
        raise ValueError("version must be a complete SemVer value")
    if workspace_version() != version:
        raise ValueError("requested version does not match Cargo.toml")
    package_version = arch_version(version)
    authorities = (
        ROOT / "packaging/PKGBUILD",
        ROOT / "packaging/aur/PKGBUILD",
        ROOT / "packaging/aur-bin/PKGBUILD",
    )
    for path in authorities:
        if assignment(path, "pkgver") != package_version:
            raise ValueError(f"{path.relative_to(ROOT)} pkgver does not match {version}")
    if assignment(ROOT / "packaging/aur/PKGBUILD", "_upstream_ver") != version:
        raise ValueError("packaging/aur/PKGBUILD _upstream_ver does not match")
    canonical_layout = package_layout(ROOT / "packaging/PKGBUILD")
    source_layout = package_layout(ROOT / "packaging/aur/PKGBUILD")
    if canonical_layout != source_layout:
        raise ValueError(
            "packaging/aur/PKGBUILD package layout has drifted from packaging/PKGBUILD"
        )
    canonical_install = (ROOT / "packaging/splinterm.install").read_bytes()
    for path in (
        ROOT / "packaging/aur/splinterm.install",
        ROOT / "packaging/aur-bin/splinterm.install",
    ):
        if path.read_bytes() != canonical_install:
            raise ValueError(
                f"{path.relative_to(ROOT)} has drifted from packaging/splinterm.install"
            )
    return package_version


def replace_assignment(text: str, name: str, value: str) -> str:
    pattern = re.compile(rf"^{re.escape(name)}=.*$", re.MULTILINE)
    replaced, count = pattern.subn(f"{name}={value}", text, count=1)
    if count != 1:
        raise ValueError(f"cannot update {name} in AUR draft")
    return replaced


def replace_checksums(text: str, checksums: list[str]) -> str:
    replacement = "sha256sums=(\n" + "".join(f"  '{value}'\n" for value in checksums) + ")"
    replaced, count = re.subn(
        r"sha256sums=\(\n(?:  '[0-9a-f]+'\n)+\)", replacement, text, count=1
    )
    if count != 1:
        raise ValueError("cannot update AUR draft checksums")
    return replaced


def validate_binary_release_urls(text: str, version: str) -> str:
    expected = f"releases/download/v{version}/"
    if text.count(expected) != 2:
        raise ValueError("AUR-bin draft does not contain exactly two versioned release URLs")
    if "releases/download/edge-" in text:
        raise ValueError("AUR-bin draft still depends on the retired edge channel")
    return text


def copy_recipe(source: Path, destination: Path) -> None:
    destination.mkdir(parents=True)
    for name in ("PKGBUILD", "splinterm.install"):
        shutil.copy2(source / name, destination / name)


def write_srcinfo(recipe: Path) -> Path:
    srcinfo = recipe / ".SRCINFO"
    srcinfo.write_text(
        run(["makepkg", "--printsrcinfo"], cwd=recipe) + "\n", encoding="utf-8"
    )
    return srcinfo


def create_source_archive(commit: str, version: str, output: Path) -> None:
    run(
        [
            "git",
            "archive",
            "--format=tar.gz",
            f"--prefix=splinterm-{version}/",
            f"--output={output}",
            commit,
        ]
    )
    listing = run(["tar", "-tzf", str(output)]).splitlines()
    prefix = f"splinterm-{version}/"
    forbidden = (f"{prefix}site/", f"{prefix}.github/workflows/site.yml")
    if any(entry.startswith(forbidden[0]) or entry == forbidden[1] for entry in listing):
        raise ValueError("source archive contains website-only release inputs")
    required = {
        f"{prefix}Cargo.toml",
        f"{prefix}packaging/PKGBUILD",
        f"{prefix}crates/splinterm/src/main.rs",
    }
    if not required.issubset(listing):
        raise ValueError("source archive is missing package build inputs")


def validate_previous_version_tag(previous: str, release_tag: str) -> str:
    if not previous.startswith("v") or SEMVER.fullmatch(previous[1:]) is None:
        raise ValueError("previous version tag must be a complete v-prefixed SemVer value")
    if previous == release_tag:
        raise ValueError("previous version tag must differ from the candidate tag")
    resolved = run(["git", "rev-parse", f"refs/tags/{previous}^{{commit}}"])
    if COMMIT.fullmatch(resolved) is None:
        raise ValueError("previous version tag does not resolve to a commit")
    return previous


def create_notes(commit: str, output: Path) -> None:
    notes = run(["git", "show", f"{commit}:RELEASE_NOTES.md"])
    if not notes:
        raise ValueError("candidate RELEASE_NOTES.md is empty")
    output.write_text(notes + "\n", encoding="utf-8")


def asset_record(path: Path, kind: str, root: Path) -> dict[str, str]:
    return {
        "kind": kind,
        "path": path.relative_to(root).as_posix(),
        "sha256": sha256(path),
    }


def validate_candidate(
    repository: str, commit: str, version: str, previous_version_tag: str
) -> tuple[str, str, str]:
    if REPOSITORY.fullmatch(repository) is None:
        raise ValueError("repository must be an owner/name pair")
    commit = commit.lower()
    if COMMIT.fullmatch(commit) is None:
        raise ValueError("commit must be a lowercase 40-character object ID")
    if run(["git", "rev-parse", f"{commit}^{{commit}}"] ) != commit:
        raise ValueError("commit does not resolve exactly")
    package_version = validate_versions(version)
    tag = f"v{version}"
    if run(["git", "tag", "--list", tag]):
        raise ValueError(f"release tag already exists: {tag}")
    previous_version_tag = validate_previous_version_tag(previous_version_tag, tag)
    return commit, package_version, previous_version_tag


def assemble(arguments: argparse.Namespace) -> dict[str, Any]:
    commit, package_version, previous_version_tag = validate_candidate(
        arguments.repository,
        arguments.commit,
        arguments.version,
        arguments.previous_version_tag,
    )
    tag = f"v{arguments.version}"
    output = arguments.output.resolve()
    if output.exists():
        shutil.rmtree(output)
    output.mkdir(parents=True)

    main_name = f"splinterm-{commit}-x86_64.pkg.tar.zst"
    mcp_name = f"splinterm-mcp-{commit}-x86_64.pkg.tar.zst"
    main_package = output / main_name
    mcp_package = output / mcp_name
    shutil.copy2(arguments.splinterm, main_package)
    shutil.copy2(arguments.splinterm_mcp, mcp_package)

    source_archive = output / f"splinterm-{arguments.version}.tar.gz"
    create_source_archive(commit, arguments.version, source_archive)

    source_recipe = output / "aur-source"
    binary_recipe = output / "aur-bin"
    copy_recipe(ROOT / "packaging/aur", source_recipe)
    copy_recipe(ROOT / "packaging/aur-bin", binary_recipe)

    source_pkgbuild = source_recipe / "PKGBUILD"
    source_text = source_pkgbuild.read_text(encoding="utf-8")
    source_text = replace_checksums(source_text, [sha256(source_archive)])
    source_pkgbuild.write_text(source_text, encoding="utf-8")

    binary_pkgbuild = binary_recipe / "PKGBUILD"
    binary_text = binary_pkgbuild.read_text(encoding="utf-8")
    binary_text = replace_assignment(binary_text, "_commit", commit)
    binary_text = validate_binary_release_urls(binary_text, arguments.version)
    binary_text = replace_checksums(
        binary_text, [sha256(main_package), sha256(mcp_package)]
    )
    binary_pkgbuild.write_text(binary_text, encoding="utf-8")
    source_srcinfo = write_srcinfo(source_recipe)
    binary_srcinfo = write_srcinfo(binary_recipe)

    notes = output / "RELEASE-NOTES.md"
    create_notes(commit, notes)
    assets = [
        asset_record(source_archive, "source-archive", output),
        asset_record(main_package, "arch-package", output),
        asset_record(mcp_package, "arch-package", output),
        asset_record(source_pkgbuild, "aur-source-draft", output),
        asset_record(source_srcinfo, "aur-source-draft", output),
        asset_record(source_recipe / "splinterm.install", "aur-source-draft", output),
        asset_record(binary_pkgbuild, "aur-bin-draft", output),
        asset_record(binary_srcinfo, "aur-bin-draft", output),
        asset_record(binary_recipe / "splinterm.install", "aur-bin-draft", output),
        asset_record(notes, "release-notes-draft", output),
    ]
    assets.sort(key=lambda record: record["path"])
    manifest = {
        "schema": SCHEMA,
        "state": "candidate",
        "publishable": False,
        "repository": arguments.repository,
        "commit": commit,
        "version": arguments.version,
        "package_version": package_version,
        "tag": tag,
        "architecture": "x86_64",
        "previous_version_tag": previous_version_tag,
        "workflow_run": arguments.workflow_run,
        "assets": assets,
    }
    manifest_path = output / "candidate-manifest.json"
    manifest_path.write_text(
        json.dumps(manifest, indent=2, sort_keys=True) + "\n", encoding="utf-8"
    )
    checksum_lines = [f"{record['sha256']}  {record['path']}" for record in assets]
    checksum_lines.append(f"{sha256(manifest_path)}  {manifest_path.name}")
    (output / "SHA256SUMS").write_text("\n".join(checksum_lines) + "\n", encoding="utf-8")
    return manifest


def parser() -> argparse.ArgumentParser:
    result = argparse.ArgumentParser(description=__doc__)
    commands = result.add_subparsers(dest="command", required=True)
    check = commands.add_parser("check")
    create = commands.add_parser("create")
    for command in (check, create):
        command.add_argument("--repository", required=True)
        command.add_argument("--commit", required=True)
        command.add_argument("--version", required=True)
        command.add_argument("--previous-version-tag", required=True)
    create.add_argument("--workflow-run", required=True)
    create.add_argument("--splinterm", type=Path, required=True)
    create.add_argument("--splinterm-mcp", type=Path, required=True)
    create.add_argument("--output", type=Path, required=True)
    return result


def main() -> int:
    arguments = parser().parse_args()
    try:
        if arguments.command == "check":
            commit, package_version, previous_version_tag = validate_candidate(
                arguments.repository,
                arguments.commit,
                arguments.version,
                arguments.previous_version_tag,
            )
            print(
                json.dumps(
                    {
                        "commit": commit,
                        "package_version": package_version,
                        "previous_version_tag": previous_version_tag,
                    }
                )
            )
        else:
            manifest = assemble(arguments)
            print(json.dumps(manifest, sort_keys=True))
    except (OSError, ValueError) as error:
        print(error, file=sys.stderr)
        return 1
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
