"""Portable capability probe for the host-targeted latency boundary."""

from __future__ import annotations

import hashlib
import pathlib
import shutil
from typing import Any

try:
    import PIL
except ImportError:
    PIL = None

REQUIRED_TOOLS = ("Hyprland", "hyprctl", "grim")


def file_sha256(path: pathlib.Path) -> str:
    digest = hashlib.sha256()
    with path.open("rb") as stream:
        for chunk in iter(lambda: stream.read(1024 * 1024), b""):
            digest.update(chunk)
    return digest.hexdigest()


def probe() -> dict[str, Any]:
    tools = {}
    for name in REQUIRED_TOOLS:
        executable = shutil.which(name)
        tools[name] = {
            "available": executable is not None,
            "executable": executable,
            "sha256": file_sha256(pathlib.Path(executable)) if executable else None,
        }
    tools["Pillow"] = {
        "available": PIL is not None,
        "executable": None,
        "sha256": None,
        "version": getattr(PIL, "__version__", None),
    }
    return {
        "schema": "splinterm.benchmark.latency-boundary-probe.v1",
        "backend": "host-hyprland-targeted-shortcut",
        "supported": all(item["available"] for item in tools.values()),
        "tools": tools,
        "input_protocol": "Hyprland hl.dsp.send_shortcut targeted window",
        "capture_protocol": "zwlr_screencopy_manager_v1 via grim",
        "presentation_protocol": None,
        "visible_boundary": "host_window_screenshot_polling_approximation",
    }
