#!/usr/bin/env python3
"""Validate the private Arch package without installing it."""

from __future__ import annotations

import argparse
import hashlib
import json
import os
from pathlib import Path
import select
import shutil
import signal
import stat
import subprocess
import sys
import tempfile
import time

REQUIRED = {
    "usr/bin/generate-omarchy-theme.py",
    "usr/bin/splinterd",
    "usr/bin/splinterm",
    "usr/bin/splinterm-relay",
    "usr/bin/splinterm-session-picker",
    "usr/bin/splinterm-pty-child",
    "usr/bin/splinterm-xdg-terminal-exec",
    "usr/lib/systemd/user/splinterd.service",
    "usr/share/applications/com.oldjobobo.splinterm.desktop",
    "usr/share/icons/hicolor/scalable/apps/com.oldjobobo.splinterm.svg",
    "usr/share/metainfo/com.oldjobobo.splinterm.metainfo.xml",
    "usr/share/doc/splinterm/automation.md",
    "usr/share/doc/splinterm/config.ini",
    "usr/share/doc/splinterm/headless.md",
    "usr/share/doc/splinterm/integrations.md",
    "usr/share/doc/splinterm/images.md",
    "usr/share/doc/splinterm/omarchy/10-splinterm.sh",
    "usr/share/doc/splinterm/packaging.md",
    "usr/share/doc/splinterm/remote.md",
    "usr/share/doc/splinterm/theme.json",
    "usr/share/doc/splinterm/xdg-terminals.list",
    "usr/share/licenses/splinterm/LICENSE",
    "usr/share/licenses/splinterm/THIRD_PARTY.md",
}
MCP_REQUIRED = {
    "usr/bin/splinterm-mcp",
    "usr/share/doc/splinterm/mcp.md",
    "usr/share/licenses/splinterm-mcp/LICENSE",
    "usr/share/licenses/splinterm-mcp/THIRD_PARTY.md",
}
EXECUTABLES = {
    "usr/bin/generate-omarchy-theme.py",
    "usr/bin/splinterd",
    "usr/bin/splinterm",
    "usr/bin/splinterm-mcp",
    "usr/bin/splinterm-relay",
    "usr/bin/splinterm-session-picker",
    "usr/bin/splinterm-pty-child",
    "usr/bin/splinterm-xdg-terminal-exec",
}
RUNTIME_DEPENDENCIES = {
    "fontconfig",
    "freetype2",
    "gcc-libs",
    "glibc",
    "hicolor-icon-theme",
    "libxkbcommon",
    "noto-fonts-cjk",
    "noto-fonts-emoji",
    "pixman",
    "python",
    "ttf-jetbrains-mono-nerd-basic",
    "wayland",
    "xdg-terminal-exec",
}


def run(command: list[str], **kwargs) -> subprocess.CompletedProcess[str]:
    kwargs.setdefault("check", True)
    kwargs.setdefault("text", True)
    return subprocess.run(command, **kwargs)


def package_entries(package: Path) -> set[str]:
    listing = run(["bsdtar", "-tf", str(package)], capture_output=True).stdout
    return {line.removeprefix("./").rstrip("/") for line in listing.splitlines() if line}


def package_metadata(package: Path) -> str:
    return run(["bsdtar", "-xOf", str(package), ".PKGINFO"], capture_output=True).stdout


def validate_launcher(root: Path) -> None:
    with tempfile.TemporaryDirectory(prefix="splinterm-launcher-") as directory:
        fake = Path(directory)
        state = fake / "state"
        record = fake / "record"
        (fake / "splinterm").write_text(
            "#!/bin/sh\n"
            "if [ \"$1\" = ping ]; then\n"
            "  count=0; [ ! -f \"$SPLINTERM_TEST_STATE/count\" ] || count=$(cat \"$SPLINTERM_TEST_STATE/count\")\n"
            "  count=$((count + 1)); printf '%s' \"$count\" >\"$SPLINTERM_TEST_STATE/count\"\n"
            "  [ -f \"$SPLINTERM_TEST_STATE/ready\" ]; exit\n"
            "fi\n"
            "printf '%s\\n' \"$@\" >\"$SPLINTERM_TEST_RECORD\"\n",
            encoding="utf-8",
        )
        (fake / "systemctl").write_text(
            "#!/bin/sh\n"
            "printf '%s\\n' \"$*\" >>\"$SPLINTERM_TEST_STATE/systemctl\"\n"
            "[ \"$2\" != restart ] || : >\"$SPLINTERM_TEST_STATE/ready\"\n",
            encoding="utf-8",
        )
        for executable in (fake / "splinterm", fake / "systemctl"):
            executable.chmod(0o755)
        state.mkdir()
        environment = os.environ.copy()
        environment.update(
            PATH=f"{fake}:{root / 'usr/bin'}:{environment['PATH']}",
            SPLINTERM_TEST_STATE=str(state),
            SPLINTERM_TEST_RECORD=str(record),
        )
        run(
            [str(root / "usr/bin/splinterm-xdg-terminal-exec"), "--working-directory", "/tmp/a b"],
            env=environment,
            timeout=10,
        )
        calls = (state / "systemctl").read_text(encoding="utf-8").splitlines()
        assert calls == ["--user start splinterd.service", "--user restart splinterd.service"]
        launch = record.read_text(encoding="utf-8").splitlines()
        assert launch[:3] == ["launch", "--new", "--name"]
        name_parts = launch[3].split("-")
        assert (
            len(name_parts) == 3
            and name_parts[0] == "terminal"
            and all(part.isdigit() for part in name_parts[1:])
        )
        assert launch[4:] == ["--working-directory", "/tmp/a b"]


def validate_systemd_unit(root: Path) -> None:
    unit = (root / "usr/lib/systemd/user/splinterd.service").read_text(encoding="utf-8")
    required = {
        "EnvironmentFile=-%h/.config/splinterm/daemon.env",
        "UnsetEnvironment=SPLINTERM_ENABLE_DEV_ATTACH",
        "ExecStart=/usr/bin/splinterd",
        "ExecReload=/usr/bin/kill -HUP $MAINPID",
        "KillSignal=SIGINT",
        "KillMode=mixed",
        "TimeoutStopSec=90",
    }
    missing = {line for line in required if line not in unit.splitlines()}
    assert not missing, f"systemd unit is missing headless safety settings: {sorted(missing)}"
    assert "graphical-session.target" not in unit
    unset_environment = {
        name
        for line in unit.splitlines()
        if line.startswith("UnsetEnvironment=")
        for name in line.partition("=")[2].split()
    }
    assert {"DISPLAY", "WAYLAND_DISPLAY"}.isdisjoint(unset_environment)
    for inherited_shell_restriction in (
        "NoNewPrivileges=",
        "PrivateTmp=",
        "RestrictSUIDSGID=",
        "UMask=",
    ):
        assert inherited_shell_restriction not in unit


def validate_headless_runtime(daemon: Path, client: Path, picker: Path) -> None:
    with tempfile.TemporaryDirectory(prefix="splinterm-headless-") as directory:
        runtime = Path(directory)
        socket = runtime / "splinterd.sock"
        policy = runtime / "policy.json"
        executable = client.resolve()
        policy.write_text(
            json.dumps({
                "schema": "splinterm.policy.v1",
                "rules": [{
                    "id": "package-headless-read",
                    "executable": {
                        "path": str(executable),
                        "sha256": hashlib.sha256(executable.read_bytes()).hexdigest(),
                    },
                    "scopes": ["topology_metadata_read"],
                    "resources": [{"kind": "lair"}],
                    "limits": {"max_results": 16},
                }],
            }),
            encoding="utf-8",
        )
        policy.chmod(0o600)
        environment = os.environ.copy()
        for name in ("DISPLAY", "WAYLAND_DISPLAY", "SPLINTERM_ENABLE_DEV_ATTACH"):
            environment.pop(name, None)
        environment.update(
            HOME=str(runtime),
            XDG_CONFIG_HOME=str(runtime / "config"),
            XDG_RUNTIME_DIR=str(runtime),
            XDG_STATE_HOME=str(runtime / "state"),
            SPLINTERM_SOCKET=str(socket),
            SPLINTERM_POLICY=str(policy),
        )
        process = subprocess.Popen(
            [str(daemon)],
            env=environment,
            stdin=subprocess.DEVNULL,
            stdout=subprocess.DEVNULL,
            stderr=subprocess.PIPE,
            text=True,
        )
        try:
            deadline = time.monotonic() + 10
            while time.monotonic() < deadline:
                if process.poll() is not None:
                    stderr = process.stderr.read() if process.stderr else ""
                    raise AssertionError(f"headless daemon exited during startup: {stderr[-2000:]}")
                if socket.exists():
                    ping = subprocess.run(
                        [str(client), "ping"],
                        env=environment,
                        capture_output=True,
                        text=True,
                        timeout=5,
                    )
                    if ping.returncode == 0:
                        break
                time.sleep(0.02)
            else:
                raise AssertionError("headless daemon did not become ready")

            listed = subprocess.run(
                [str(client), "list"],
                env=environment,
                capture_output=True,
                text=True,
                timeout=10,
            )
            assert listed.returncode == 0, listed.stderr

            picked = subprocess.run(
                [sys.executable, str(picker), "--splinterm", str(client), "list"],
                env=environment,
                capture_output=True,
                text=True,
                timeout=10,
            )
            assert picked.returncode == 0, picked.stderr
            assert picked.stdout == ""

            spoofed = environment.copy()
            spoofed.update(
                SPLINTERM_DOJO_ID="018f4d8c-2a18-4b31-8c2f-9e7c5de77101",
                SPLINTERM_WINDOW_ID="018f4d8c-2a18-4b31-8c2f-9e7c5de77102",
                SPLINTERM_SPLINT_ID="018f4d8c-2a18-4b31-8c2f-9e7c5de77103",
                SPLINTERM_SPLINT_INCARNATION="1",
            )
            denied = subprocess.run(
                [
                    str(client),
                    "--output",
                    "json",
                    "--schema-major",
                    "1",
                    "--timeout-ms",
                    "5000",
                    "new",
                    "context-is-not-authority",
                    "--",
                    "/bin/true",
                ],
                env=spoofed,
                capture_output=True,
                text=True,
                timeout=10,
            )
            assert denied.returncode == 3, (denied.stdout, denied.stderr)
            denied_document = json.loads(denied.stdout)
            assert denied_document["ok"] is False
            assert denied_document["error"]["code"] == "unauthorized"
        finally:
            if process.poll() is None:
                process.send_signal(signal.SIGINT)
                try:
                    process.wait(timeout=90)
                except subprocess.TimeoutExpired:
                    process.kill()
                    process.wait(timeout=5)
        assert process.returncode == 0
        assert not socket.exists(), "headless daemon left its socket after clean shutdown"


def validate_picker_runtime(daemon: Path, client: Path, picker: Path) -> None:
    with tempfile.TemporaryDirectory(prefix="splinterm-picker-runtime-") as directory:
        runtime = Path(directory)
        socket = runtime / "splinterd.sock"
        policy = runtime / "policy.json"
        executable = client.resolve()
        identity = {
            "path": str(executable),
            "sha256": hashlib.sha256(executable.read_bytes()).hexdigest(),
        }

        def install_rule(
            rule_id: str,
            scopes: list[str],
            resources: list[dict[str, object]],
            max_spawn_count: int | None = None,
        ) -> None:
            limits: dict[str, int] = {
                "max_returned_rows": 16,
                "max_returned_bytes": 8 * 1024 * 1024,
                "max_live_subscriptions": 1,
            }
            if max_spawn_count is not None:
                limits["max_spawn_count"] = max_spawn_count
            policy.write_text(
                json.dumps({
                    "schema": "splinterm.policy.v1",
                    "rules": [{
                        "id": rule_id,
                        "executable": identity,
                        "scopes": scopes,
                        "resources": resources,
                        "limits": limits,
                    }],
                }),
                encoding="utf-8",
            )
            policy.chmod(0o600)

        install_rule(
            "picker-bootstrap",
            ["topology_metadata_read", "process_spawn", "topology_layout_mutate"],
            [{"kind": "lair"}],
            1,
        )
        environment = os.environ.copy()
        for name in ("DISPLAY", "WAYLAND_DISPLAY", "SPLINTERM_ENABLE_DEV_ATTACH"):
            environment.pop(name, None)
        environment.update(
            HOME=str(runtime),
            XDG_CONFIG_HOME=str(runtime / "config"),
            XDG_RUNTIME_DIR=str(runtime),
            XDG_STATE_HOME=str(runtime / "state"),
            SPLINTERM_SOCKET=str(socket),
            SPLINTERM_POLICY=str(policy),
        )
        process = subprocess.Popen(
            [str(daemon)],
            env=environment,
            stdin=subprocess.DEVNULL,
            stdout=subprocess.DEVNULL,
            stderr=subprocess.PIPE,
            text=True,
        )

        def machine(arguments: list[str], env: dict[str, str] = environment) -> subprocess.CompletedProcess[str]:
            return subprocess.run(
                [
                    str(client),
                    "--output",
                    "json",
                    "--schema-major",
                    "1",
                    "--timeout-ms",
                    "10000",
                    *arguments,
                ],
                env=env,
                capture_output=True,
                text=True,
                timeout=15,
            )

        try:
            deadline = time.monotonic() + 10
            while time.monotonic() < deadline:
                if process.poll() is not None:
                    stderr = process.stderr.read() if process.stderr else ""
                    raise AssertionError(f"picker daemon exited during startup: {stderr[-2000:]}")
                if socket.exists():
                    ping = subprocess.run(
                        [str(client), "ping"],
                        env=environment,
                        capture_output=True,
                        text=True,
                        timeout=5,
                    )
                    if ping.returncode == 0:
                        break
                time.sleep(0.02)
            else:
                raise AssertionError("picker daemon did not become ready")

            created = machine([
                "new",
                "picker-reference",
                "--",
                "/bin/sh",
                "-c",
                "printf 'picker-parent-ready\\n'; exec sleep 30",
            ])
            assert created.returncode == 0, (created.stdout, created.stderr)
            parent = json.loads(created.stdout)["resource"]
            context = environment.copy()
            context.update(
                SPLINTERM_DOJO_ID=parent["dojo_id"],
                SPLINTERM_WINDOW_ID=parent["window_id"],
                SPLINTERM_SPLINT_ID=parent["splint_id"],
                SPLINTERM_SPLINT_INCARNATION=str(parent["incarnation"]),
            )
            install_rule(
                "picker-split",
                ["topology_metadata_read", "process_spawn", "topology_layout_mutate"],
                [
                    {"kind": "lair"},
                    {
                        "kind": "splint",
                        "splint_id": parent["splint_id"],
                        "incarnation": parent["incarnation"],
                    },
                ],
                1,
            )
            process.send_signal(signal.SIGHUP)
            time.sleep(0.2)

            selected = subprocess.run(
                [sys.executable, str(picker), "--splinterm", str(client), "context"],
                env=context,
                capture_output=True,
                text=True,
                timeout=15,
            )
            assert selected.returncode == 0, (selected.stdout, selected.stderr)
            assert json.loads(selected.stdout)["incarnation"] == parent["incarnation"]

            split = subprocess.run(
                [
                    sys.executable,
                    str(picker),
                    "--splinterm",
                    str(client),
                    "split-context",
                    "--axis",
                    "vertical",
                    "--side",
                    "second",
                    "--",
                    "/bin/sh",
                    "-c",
                    "printf 'picker-child-ready\\n'; exec sleep 30",
                ],
                env=context,
                capture_output=True,
                text=True,
                timeout=15,
            )
            assert split.returncode == 0, (split.stdout, split.stderr)
            child = json.loads(split.stdout)["resource"]
            child_context = context.copy()
            child_context.update(
                SPLINTERM_DOJO_ID=child["dojo_id"],
                SPLINTERM_WINDOW_ID=child["window_id"],
                SPLINTERM_SPLINT_ID=child["splint_id"],
                SPLINTERM_SPLINT_INCARNATION=str(child["incarnation"]),
            )
            install_rule(
                "picker-read-child",
                ["topology_metadata_read", "terminal_visible_read", "terminal_subscribe"],
                [
                    {"kind": "lair"},
                    {
                        "kind": "splint",
                        "splint_id": child["splint_id"],
                        "incarnation": child["incarnation"],
                    },
                ],
            )
            process.send_signal(signal.SIGHUP)
            time.sleep(0.2)

            snapshot = subprocess.run(
                [
                    sys.executable,
                    str(picker),
                    "--splinterm",
                    str(client),
                    "snapshot-context",
                ],
                env=child_context,
                capture_output=True,
                text=True,
                timeout=15,
            )
            assert snapshot.returncode == 0, (snapshot.stdout, snapshot.stderr)
            snapshot_document = json.loads(snapshot.stdout)
            text = "".join(
                cell["text"]
                for row in snapshot_document["data"]["rows"]
                for cell in row["cells"]
            )
            assert "picker-child-ready" in text

            denied = subprocess.run(
                [
                    sys.executable,
                    str(picker),
                    "--splinterm",
                    str(client),
                    "send-context",
                    "literal-input",
                ],
                env=child_context,
                capture_output=True,
                text=True,
                timeout=15,
            )
            assert denied.returncode == 3, (denied.stdout, denied.stderr)
            assert denied.stdout == ""
            assert "unauthorized" in denied.stderr
        finally:
            if process.poll() is None:
                process.send_signal(signal.SIGINT)
                try:
                    process.wait(timeout=90)
                except subprocess.TimeoutExpired:
                    process.kill()
                    process.wait(timeout=5)
        assert process.returncode == 0
        assert not socket.exists(), "picker daemon left its socket after clean shutdown"

def encode_private_frame(document: dict[str, object]) -> bytes:
    body = json.dumps(document, separators=(",", ":")).encode("utf-8")
    assert 0 < len(body) <= 8 * 1024 * 1024
    return len(body).to_bytes(4, "big") + body


def read_exact_fd(descriptor: int, length: int, timeout: float = 10) -> bytes:
    deadline = time.monotonic() + timeout
    chunks = bytearray()
    while len(chunks) < length:
        remaining = deadline - time.monotonic()
        assert remaining > 0, "timed out reading relay output"
        readable, _, _ = select.select([descriptor], [], [], remaining)
        assert readable, "timed out reading relay output"
        chunk = os.read(descriptor, length - len(chunks))
        assert chunk, "relay output closed during a private frame"
        chunks.extend(chunk)
    return bytes(chunks)


def read_private_frame(pipe) -> dict[str, object]:
    descriptor = pipe.fileno()
    length = int.from_bytes(read_exact_fd(descriptor, 4), "big")
    assert 0 < length <= 8 * 1024 * 1024
    return json.loads(read_exact_fd(descriptor, length))


def validate_relay_runtime(daemon: Path, client: Path, relay: Path) -> None:
    with tempfile.TemporaryDirectory(prefix="splinterm-relay-package-") as directory:
        runtime = Path(directory)
        socket = runtime / "splinterd.sock"
        policy = runtime / "policy.json"
        executable = relay.resolve()
        policy.write_text(
            json.dumps({
                "schema": "splinterm.policy.v1",
                "rules": [{
                    "id": "package-relay-read",
                    "executable": {
                        "path": str(executable),
                        "sha256": hashlib.sha256(executable.read_bytes()).hexdigest(),
                    },
                    "scopes": ["topology_metadata_read", "topology_subscribe"],
                    "resources": [{"kind": "lair"}],
                    "limits": {"max_results": 16, "max_live_subscriptions": 1},
                }],
            }),
            encoding="utf-8",
        )
        policy.chmod(0o600)
        environment = os.environ.copy()
        for name in ("DISPLAY", "WAYLAND_DISPLAY", "SPLINTERM_ENABLE_DEV_ATTACH"):
            environment.pop(name, None)
        environment.update(
            HOME=str(runtime),
            XDG_CONFIG_HOME=str(runtime / "config"),
            XDG_RUNTIME_DIR=str(runtime),
            XDG_STATE_HOME=str(runtime / "state"),
            SPLINTERM_SOCKET=str(socket),
            SPLINTERM_POLICY=str(policy),
        )
        daemon_process = subprocess.Popen(
            [str(daemon)],
            env=environment,
            stdin=subprocess.DEVNULL,
            stdout=subprocess.DEVNULL,
            stderr=subprocess.PIPE,
            text=True,
        )
        relay_process = None
        try:
            deadline = time.monotonic() + 10
            while time.monotonic() < deadline and not socket.exists():
                assert daemon_process.poll() is None, "relay test daemon exited during startup"
                time.sleep(0.02)
            assert socket.exists(), "relay test daemon did not create its socket"
            inherited = open("/dev/null", "rb")
            os.set_inheritable(inherited.fileno(), True)
            inherited_fd = inherited.fileno()
            inherited_target = os.readlink(f"/proc/self/fd/{inherited_fd}")
            relay_process = subprocess.Popen(
                [str(client), "relay", "--stdio"],
                env=environment,
                stdin=subprocess.PIPE,
                stdout=subprocess.PIPE,
                stderr=subprocess.PIPE,
                pass_fds=(inherited_fd,),
            )
            inherited.close()
            assert relay_process.stdin is not None
            assert relay_process.stdout is not None
            relay_process.stdin.write(encode_private_frame({
                "type": "hello",
                "minimum_version": 23,
                "maximum_version": 23,
                "role": "automation",
            }))
            relay_process.stdin.flush()
            hello = read_private_frame(relay_process.stdout)
            assert hello["type"] == "hello" and hello["version"] == 24
            inherited_path = Path(f"/proc/{relay_process.pid}/fd/{inherited_fd}")
            assert not inherited_path.exists() or os.readlink(inherited_path) != inherited_target

            relay_process.stdin.write(encode_private_frame({
                "type": "request",
                "request_id": 1,
                "request": {"type": "inspect_topology"},
            }))
            relay_process.stdin.flush()
            topology = read_private_frame(relay_process.stdout)
            assert topology["type"] == "response" and topology["request_id"] == 1

            relay_process.stdin.write(encode_private_frame({
                "type": "request",
                "request_id": 2,
                "request": {
                    "type": "create_dojo",
                    "expected_topology_revision": 0,
                    "name": "must-be-denied",
                    "launch": {
                        "cwd": str(runtime),
                        "command": ["/bin/true"],
                        "shell": None,
                        "login_shell": False,
                        "scrollback_lines": 100,
                    },
                },
            }))
            relay_process.stdin.flush()
            denied = read_private_frame(relay_process.stdout)
            assert denied["type"] == "error"
            assert denied["request_id"] == 2
            assert denied["error"]["code"] == "unauthorized"

            relay_process.stdin.write(encode_private_frame({
                "type": "request",
                "request_id": 3,
                "request": {"type": "subscribe_topology"},
            }))
            relay_process.stdin.flush()
            subscribed = read_private_frame(relay_process.stdout)
            assert subscribed["type"] == "response" and subscribed["request_id"] == 3, subscribed

            relay_process.kill()
            relay_process.wait(timeout=10)
            assert daemon_process.poll() is None

            relay_process = subprocess.Popen(
                [str(client), "relay", "--stdio"],
                env=environment,
                stdin=subprocess.PIPE,
                stdout=subprocess.PIPE,
                stderr=subprocess.PIPE,
            )
            assert relay_process.stdin is not None
            assert relay_process.stdout is not None
            relay_process.stdin.write(encode_private_frame({
                "type": "hello",
                "minimum_version": 23,
                "maximum_version": 23,
                "role": "automation",
            }))
            relay_process.stdin.flush()
            assert read_private_frame(relay_process.stdout)["type"] == "hello"
            relay_process.stdin.write(encode_private_frame({
                "type": "cancel",
                "request_id": 30,
            }))
            relay_process.stdin.flush()
            cancelled = read_private_frame(relay_process.stdout)
            assert cancelled["type"] == "error" and cancelled["request_id"] == 30
            assert cancelled["error"]["code"] == "request_not_found"
            relay_process.stdin.write(encode_private_frame({
                "type": "request",
                "request_id": 31,
                "request": {"type": "subscribe_topology"},
            }))
            relay_process.stdin.flush()
            replacement_subscription = read_private_frame(relay_process.stdout)
            assert replacement_subscription["type"] == "response"
            assert replacement_subscription["request_id"] == 31

            daemon_process.send_signal(signal.SIGINT)
            daemon_process.wait(timeout=90)
            assert daemon_process.returncode == 0
            relay_process.wait(timeout=10)
            assert relay_process.returncode == 0
            stderr = relay_process.stderr.read() if relay_process.stderr else b""
            assert not stderr
            assert not socket.exists()

            daemon_process = subprocess.Popen(
                [str(daemon)],
                env=environment,
                stdin=subprocess.DEVNULL,
                stdout=subprocess.DEVNULL,
                stderr=subprocess.PIPE,
                text=True,
            )
            deadline = time.monotonic() + 10
            while time.monotonic() < deadline and not socket.exists():
                assert daemon_process.poll() is None, "restarted relay test daemon exited"
                time.sleep(0.02)
            assert socket.exists(), "restarted relay test daemon did not create its socket"

            restarted_relay = subprocess.Popen(
                [str(client), "relay", "--stdio"],
                env=environment,
                stdin=subprocess.PIPE,
                stdout=subprocess.PIPE,
                stderr=subprocess.PIPE,
            )
            assert restarted_relay.stdin is not None
            assert restarted_relay.stdout is not None
            restarted_relay.stdin.write(encode_private_frame({
                "type": "hello",
                "minimum_version": 23,
                "maximum_version": 23,
                "role": "automation",
            }))
            restarted_relay.stdin.flush()
            assert read_private_frame(restarted_relay.stdout)["type"] == "hello"
            restarted_relay.stdin.close()
            restarted_relay.wait(timeout=10)
            assert restarted_relay.returncode == 0
            assert not (restarted_relay.stderr.read() if restarted_relay.stderr else b"")

            malformed_relay = subprocess.Popen(
                [str(client), "relay", "--stdio"],
                env=environment,
                stdin=subprocess.PIPE,
                stdout=subprocess.PIPE,
                stderr=subprocess.PIPE,
            )
            assert malformed_relay.stdin is not None
            malformed_relay.stdin.write((8 * 1024 * 1024 + 1).to_bytes(4, "big"))
            malformed_relay.stdin.flush()
            malformed_relay.stdin.close()
            malformed_relay.wait(timeout=10)
            assert malformed_relay.returncode == 0
            assert not (malformed_relay.stderr.read() if malformed_relay.stderr else b"")

            broken_output_relay = subprocess.Popen(
                [str(client), "relay", "--stdio"],
                env=environment,
                stdin=subprocess.PIPE,
                stdout=subprocess.PIPE,
                stderr=subprocess.PIPE,
            )
            assert broken_output_relay.stdin is not None
            assert broken_output_relay.stdout is not None
            broken_output_relay.stdin.write(encode_private_frame({
                "type": "hello",
                "minimum_version": 23,
                "maximum_version": 23,
                "role": "automation",
            }))
            broken_output_relay.stdin.flush()
            broken_output_relay.stdout.close()
            broken_output_relay.wait(timeout=10)
            broken_output_relay.stdin.close()
            assert broken_output_relay.returncode == 1
            broken_stderr = (
                broken_output_relay.stderr.read() if broken_output_relay.stderr else b""
            )
            assert b"daemon-to-stdout relay failed" in broken_stderr
        finally:
            if relay_process is not None and relay_process.poll() is None:
                relay_process.kill()
                relay_process.wait(timeout=5)
            if daemon_process.poll() is None:
                daemon_process.send_signal(signal.SIGINT)
                try:
                    daemon_process.wait(timeout=90)
                except subprocess.TimeoutExpired:
                    daemon_process.kill()
                    daemon_process.wait(timeout=5)
        assert daemon_process.returncode == 0
        assert not socket.exists(), "relay test daemon left its socket after shutdown"


def validate_theme_generator(root: Path) -> None:
    colors = {
        "accent": "#010203", "bg": "#101112", "darker_bg": "#000000",
        "selection": "#202122", "muted": "#303132", "fg": "#d0d1d2",
        "bright_fg": "#ffffff", "red": "#800000", "yellow": "#808000",
        "green": "#008000", "cyan": "#008080", "blue": "#000080",
        "magenta": "#800080", "bright_red": "#ff0000",
        "bright_yellow": "#ffff00", "bright_green": "#00ff00",
        "bright_cyan": "#00ffff", "bright_blue": "#0000ff",
        "bright_magenta": "#ff00ff",
    }
    with tempfile.TemporaryDirectory(prefix="splinterm-theme-") as directory:
        directory = Path(directory)
        source = directory / "colors.toml"
        source.write_text("".join(f'{key} = "{value}"\n' for key, value in colors.items()))
        (directory / "foot.ini").write_text("[colors-dark]\nalpha=0.85\n")
        output = directory / "theme.json"
        run([str(root / "usr/bin/generate-omarchy-theme.py"), str(source), "--output", str(output)])
        generated = json.loads(output.read_text(encoding="utf-8"))
        assert generated["background"] == colors["bg"]
        assert generated["cursor"] == colors["accent"]
        assert generated["alpha"] == 0.85
        assert len(generated["ansi"]) == 16


def main() -> int:
    parser = argparse.ArgumentParser()
    parser.add_argument("package", type=Path)
    parser.add_argument("--mcp-package", required=True, type=Path)
    args = parser.parse_args()
    package = args.package.resolve()
    mcp_package = args.mcp_package.resolve()
    for candidate in (package, mcp_package):
        if not candidate.is_file() or candidate.stat().st_size > 256 * 1024 * 1024:
            raise SystemExit(f"package is missing or unexpectedly large: {candidate}")

    entries = package_entries(package)
    mcp_entries = package_entries(mcp_package)
    missing = REQUIRED - entries
    assert not missing, f"missing package paths: {sorted(missing)}"
    missing_mcp = MCP_REQUIRED - mcp_entries
    assert not missing_mcp, f"missing MCP package paths: {sorted(missing_mcp)}"
    assert "usr/bin/splinterm-mcp" not in entries, "MCP adapter must remain an opt-in split package"
    forbidden = [
        entry for entry in entries | mcp_entries
        if entry.startswith(("home/", "etc/", "usr/share/omarchy/"))
    ]
    assert not forbidden, f"package mutates forbidden paths: {forbidden}"

    with tempfile.TemporaryDirectory(prefix="splinterm-package-root-") as directory:
        root = Path(directory)
        run(["bsdtar", "-xf", str(package), "-C", str(root)])
        run(["bsdtar", "-xf", str(mcp_package), "-C", str(root)])
        for relative in EXECUTABLES:
            mode = (root / relative).stat().st_mode
            assert mode & stat.S_IXUSR, f"{relative} is not executable"
        for binary in ("splinterm", "splinterd", "splinterm-mcp", "splinterm-relay", "splinterm-pty-child"):
            result = run(["ldd", str(root / "usr/bin" / binary)], capture_output=True)
            assert "not found" not in result.stdout

        desktop = root / "usr/share/applications/com.oldjobobo.splinterm.desktop"
        metadata = root / "usr/share/metainfo/com.oldjobobo.splinterm.metainfo.xml"
        run(["desktop-file-validate", str(desktop)])
        run(["appstreamcli", "validate", "--no-net", str(metadata)])
        assert "Exec=splinterm-xdg-terminal-exec" in desktop.read_text(encoding="utf-8")
        assert "Icon=com.oldjobobo.splinterm" in desktop.read_text(encoding="utf-8")

        pkginfo = package_metadata(package)
        dependencies = {
            line.split(" = ", 1)[1].split(">", 1)[0].split("<", 1)[0].split("=", 1)[0]
            for line in pkginfo.splitlines() if line.startswith("depend = ")
        }
        assert RUNTIME_DEPENDENCIES <= dependencies
        mcp_pkginfo = package_metadata(mcp_package)
        assert "pkgname = splinterm-mcp" in mcp_pkginfo
        assert any(
            line.startswith("depend = splinterm=")
            for line in mcp_pkginfo.splitlines()
        ), "MCP split package must depend on the exact main package version"
        validate_systemd_unit(root)
        validate_headless_runtime(
            root / "usr/bin/splinterd",
            root / "usr/bin/splinterm",
            root / "usr/bin/splinterm-session-picker",
        )
        validate_picker_runtime(
            root / "usr/bin/splinterd",
            root / "usr/bin/splinterm",
            root / "usr/bin/splinterm-session-picker",
        )
        validate_relay_runtime(
            root / "usr/bin/splinterd",
            root / "usr/bin/splinterm",
            root / "usr/bin/splinterm-relay",
        )
        validate_theme_generator(root)
        validate_launcher(root)
        run([sys.executable, str(Path(__file__).with_name("validate-mcp-package.py")), str(root)], timeout=180)

    print(f"Package validation passed: {package} + {mcp_package}")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
