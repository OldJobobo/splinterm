#!/usr/bin/env python3
"""Run deterministic release-state checks before compilation or publication."""

from __future__ import annotations

import argparse
from pathlib import Path
import re
import shutil
import subprocess
import sys
from urllib.parse import unquote

ROOT = Path(__file__).resolve().parents[2]
SEMVER = re.compile(r"[0-9]+\.[0-9]+\.[0-9]+(?:-[0-9A-Za-z.-]+)?")
LINK = re.compile(r"(?<!!)\[[^]]*\]\(([^)]+)\)")
PROHIBITED = ("splinterm-brain", "/home/oldjobobo/", "plans/0048-")


def run(arguments: list[str], *, cwd: Path) -> str:
    result = subprocess.run(
        arguments, cwd=cwd, text=True, capture_output=True, check=False
    )
    if result.returncode != 0:
        detail = result.stderr.strip() or result.stdout.strip()
        raise ValueError(f"command failed ({' '.join(arguments)}): {detail}")
    return result.stdout


def assignment(path: Path, name: str) -> str:
    match = re.search(
        rf"^{re.escape(name)}=([^\n]+)$", path.read_text(encoding="utf-8"), re.MULTILINE
    )
    if match is None:
        raise ValueError(f"{path} does not define {name}")
    return match.group(1).strip().strip("'\"")


def workspace_version(root: Path) -> str:
    text = (root / "Cargo.toml").read_text(encoding="utf-8")
    section = text.split("[workspace.package]", 1)
    if len(section) != 2:
        raise ValueError("Cargo.toml has no [workspace.package] section")
    match = re.search(r'^version\s*=\s*"([^"]+)"$', section[1], re.MULTILINE)
    if match is None:
        raise ValueError("workspace version is unavailable")
    return match.group(1)


def check_versions(root: Path) -> list[str]:
    errors: list[str] = []
    try:
        version = workspace_version(root)
        if SEMVER.fullmatch(version) is None:
            errors.append(f"workspace version is not complete SemVer: {version}")
            return errors
        package_version = version.replace("-", "")
        for relative in (
            "packaging/PKGBUILD",
            "packaging/aur/PKGBUILD",
            "packaging/aur-bin/PKGBUILD",
        ):
            actual = assignment(root / relative, "pkgver")
            if actual != package_version:
                errors.append(f"{relative} pkgver {actual!r} != {package_version!r}")
        upstream = assignment(root / "packaging/aur/PKGBUILD", "_upstream_ver")
        if upstream != version:
            errors.append("packaging/aur/PKGBUILD _upstream_ver != workspace version")
    except (OSError, ValueError) as error:
        errors.append(str(error))
    return errors


def check_workflows(root: Path) -> list[str]:
    try:
        ci = (root / ".github/workflows/ci.yml").read_text(encoding="utf-8")
        candidate = (root / ".github/workflows/release-candidate.yml").read_text(
            encoding="utf-8"
        )
        pinned_foot = (root / ".github/workflows/foot-oracle-pinned.yml").read_text(
            encoding="utf-8"
        )
        promotion = (root / ".github/workflows/promote-release.yml").read_text(
            encoding="utf-8"
        )
        recovery = (root / ".github/workflows/recover-release.yml").read_text(
            encoding="utf-8"
        )
        aur = (root / ".github/workflows/distribute-aur.yml").read_text(
            encoding="utf-8"
        )
    except OSError as error:
        return [f"release workflow unavailable: {error}"]
    requirements = {
        "CI authority branches": ('branches: [main, "maint/0.1"]', ci),
        "CI fail-closed aggregator": ("if: ${{ always() }}", ci),
        "CI explicit dependency result checks": ("needs.preflight.result == 'success'", ci),
        "standalone Foot manual trigger": ("workflow_dispatch:", pinned_foot),
        "standalone Foot schedule": ("schedule:", pinned_foot),
        "standalone portable Foot runner": ("runs-on: ubuntu-latest", pinned_foot),
        "standalone portable Foot validation": (
            "python tools/foot-oracle/check-provenance.py --portable",
            pinned_foot,
        ),
        "standalone pinned Foot runner": (
            "runs-on: [self-hosted, linux, x64, splinterm-oracle]",
            pinned_foot,
        ),
        "candidate authority branches": ("refs/heads/main|refs/heads/maint/0.1", candidate),
        "candidate Actions read permission": ("actions: read", candidate),
        "candidate read-only contents": ("contents: read", candidate),
        "promotion authority branches": ("refs/heads/main|refs/heads/maint/0.1", promotion),
        "promotion protected environment": ("environment: release", promotion),
        "pinned executable actionlint": (
            "docker://rhysd/actionlint@sha256:887a259a5a534f3c4f36cb02dca341673c6089431057242cdc931e9f133147e9",
            ci,
        ),
        "recovery exact promotion digest": ("promotion_record_sha256:", recovery),
        "recovery protected environment": ("environment: release", recovery),
        "AUR exact publication receipt digest": ("publication_receipt_sha256:", aur),
        "AUR protected environment": ("environment: aur-release", aur),
        "AUR pinned SSH host identity": ("aur.archlinux.org ssh-ed25519", aur),
    }
    errors = [
        f"release authority configuration missing {label}"
        for label, (token, text) in requirements.items()
        if token not in text
    ]
    try:
        check = ci[ci.index("  check:") :]
    except ValueError:
        errors.append("CI aggregate check boundary is malformed")
    else:
        if "renderer-contracts" not in check:
            errors.append("CI aggregate omits Splinterm-owned renderer contracts")
    if (
        "foot-reference" in ci
        or "tools/foot-oracle/check-provenance.py --portable" in ci
        or "tools/foot-oracle/test_*.py" in ci
    ):
        errors.append("release-authority CI includes historical Foot advisory tooling")
    if "self-hosted" in ci:
        errors.append("release-authority CI includes a self-hosted runner")
    if "\n  push:" in pinned_foot or "\n  pull_request:" in pinned_foot:
        errors.append("pinned Foot workflow runs automatically on release-authority changes")
    if "tools/foot-oracle/check-provenance.py" in candidate:
        errors.append("candidate construction depends on historical Foot provenance")
    return errors


def markdown_files(root: Path) -> list[Path]:
    result = subprocess.run(
        ["git", "ls-files", "*.md"],
        cwd=root,
        text=True,
        capture_output=True,
        check=False,
    )
    if result.returncode == 0:
        return [root / line for line in result.stdout.splitlines() if line]
    return sorted(root.rglob("*.md"))


def check_markdown(root: Path) -> list[str]:
    errors: list[str] = []
    for path in markdown_files(root):
        text = path.read_text(encoding="utf-8")
        relative = path.relative_to(root)
        for prohibited in PROHIBITED:
            if prohibited in text:
                errors.append(f"{relative} contains prohibited private path {prohibited!r}")
        for raw in LINK.findall(text):
            target = raw.strip().split(maxsplit=1)[0].strip("<>")
            if not target or target.startswith(("#", "/", "http://", "https://", "mailto:")):
                continue
            target = unquote(target.split("#", 1)[0].split("?", 1)[0])
            candidate = (path.parent / target).resolve()
            try:
                candidate.relative_to(root.resolve())
            except ValueError:
                errors.append(f"{relative} link escapes repository: {raw}")
                continue
            if not candidate.exists():
                errors.append(f"{relative} has missing local link: {raw}")
    return errors


def check_generated_metadata(root: Path) -> list[str]:
    if shutil.which("makepkg") is None:
        return []
    errors: list[str] = []
    for relative in (Path("packaging/aur"), Path("packaging/aur-bin")):
        try:
            generated = run(["makepkg", "--printsrcinfo"], cwd=root / relative)
            checked_in = (root / relative / ".SRCINFO").read_text(encoding="utf-8")
            if checked_in != generated:
                errors.append(f"{relative}/.SRCINFO is stale; regenerate with makepkg --printsrcinfo")
        except (OSError, ValueError) as error:
            errors.append(str(error))
    return errors


def check_candidate_range(root: Path, version: str | None) -> list[str]:
    if version is None:
        return []
    errors: list[str] = []
    if SEMVER.fullmatch(version) is None:
        return ["candidate version must be complete SemVer"]
    if version != workspace_version(root):
        errors.append("candidate version does not match Cargo.toml")
    tag = f"v{version}"
    if run(["git", "tag", "--list", tag], cwd=root).strip():
        errors.append(f"candidate tag already exists: {tag}")
    tags = run(["git", "tag", "--sort=-version:refname", "--merged", "HEAD"], cwd=root).splitlines()
    predecessors = [value for value in tags if value.startswith("v") and SEMVER.fullmatch(value[1:])]
    if predecessors:
        range_spec = f"{predecessors[0]}..HEAD"
        if not run(["git", "log", "--format=%H", range_spec], cwd=root).strip():
            errors.append(f"release-note range is empty: {range_spec}")
    return errors


def diagnose(root: Path, *, version: str | None = None, generated: bool = True) -> list[str]:
    checks = [
        check_versions(root),
        check_workflows(root),
        check_markdown(root),
        check_candidate_range(root, version),
    ]
    if generated:
        checks.append(check_generated_metadata(root))
    return [error for group in checks for error in group]


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--version", help="candidate SemVer; also validates tag/range state")
    parser.add_argument(
        "--skip-generated-metadata",
        action="store_true",
        help="skip makepkg-based .SRCINFO comparison even when makepkg is available",
    )
    arguments = parser.parse_args()
    try:
        errors = diagnose(
            ROOT,
            version=arguments.version,
            generated=not arguments.skip_generated_metadata,
        )
    except (OSError, ValueError) as error:
        errors = [str(error)]
    if errors:
        for error in errors:
            print(f"release doctor: ERROR: {error}", file=sys.stderr)
        return 1
    print("release doctor: release state is consistent")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
