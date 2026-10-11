"""Non-graphical installed-package check; never touches a live user daemon."""

import os
from pathlib import Path
import signal
import subprocess
import sys
import tempfile
import time


def main():
    package = Path(sys.argv[1])
    with tempfile.TemporaryDirectory(prefix="splinterm-nix-") as directory:
        root = Path(directory)
        runtime = root / "runtime"
        runtime.mkdir(mode=0o700)
        environment = {
            **{key: value for key, value in os.environ.items()
               if not key.startswith("SPLINTERM_") and key not in ("WAYLAND_DISPLAY", "DISPLAY")},
            "HOME": str(root),
            "XDG_CONFIG_HOME": str(root / "config"),
            "XDG_STATE_HOME": str(root / "state"),
            "XDG_CACHE_HOME": str(root / "cache"),
            "XDG_RUNTIME_DIR": str(runtime),
            "SPLINTERM_SOCKET": str(runtime / "daemon.sock"),
        }
        with (root / "daemon.log").open("w+") as log:
            daemon = subprocess.Popen([str(package / "bin/splinterd")], env=environment,
                                      stdout=log, stderr=log)
            try:
                deadline = time.monotonic() + 10
                while not (runtime / "daemon.sock").exists():
                    if daemon.poll() is not None or time.monotonic() >= deadline:
                        log.seek(0)
                        raise RuntimeError(f"private daemon did not start: {log.read()}")
                    time.sleep(0.05)
                # Human mode deliberately exercises trusted sibling authentication.
                for command in ("ping", "list"):
                    result = subprocess.run([str(package / "bin/splinterm"), command],
                                            env=environment, capture_output=True, text=True,
                                            timeout=10, check=False)
                    if result.returncode:
                        raise RuntimeError(f"{command}: {result.stdout}\n{result.stderr}")
                marker = root / "pty-ready"
                result = subprocess.run(
                    [str(package / "bin/splinterm"), "new", "package-smoke",
                     "--cwd", str(root), "--", sys.argv[2], "-c",
                     'printf ready > "$1"', "smoke", str(marker)],
                    env=environment, capture_output=True, text=True, timeout=10, check=False,
                )
                if result.returncode:
                    raise RuntimeError(f"PTY launch: {result.stdout}\n{result.stderr}")
                deadline = time.monotonic() + 10
                while not marker.exists():
                    if time.monotonic() >= deadline:
                        raise RuntimeError("installed PTY helper did not execute its command")
                    time.sleep(0.05)
                if marker.read_text() != "ready":
                    raise RuntimeError("installed PTY command produced unexpected output")
            finally:
                if daemon.poll() is None:
                    daemon.send_signal(signal.SIGINT)
                    try:
                        daemon.wait(timeout=10)
                    except subprocess.TimeoutExpired:
                        daemon.kill()
                        daemon.wait()
                        raise RuntimeError("private daemon did not stop cleanly") from None
            if daemon.returncode != 0:
                log.seek(0)
                raise RuntimeError(f"private daemon failed: {log.read()}")
            if (runtime / "daemon.sock").exists():
                raise RuntimeError("private daemon left its socket behind")
    print("Installed package: trusted sibling CLI, PTY helper, and clean private daemon shutdown passed")


if __name__ == "__main__":
    main()
