# Phase 9 private packaging evidence

The private `0.1.0.pre-1` Arch package was rebuilt from commit `d1622e9` with
`tools/package/build-local-package.sh`. `makepkg` compiled release binaries and
ran workspace tests inside the committed source archive. The package was not
installed or published.

`tools/package/validate-package.py` extracted an isolated package root and
verified owned paths, executable modes, resolved ELF libraries, declared
runtime dependencies, desktop/AppStream metadata, icon/service/helper layout,
theme generation, forbidden-path absence, and simulated stale-protocol service
restart. A separate extracted-root smoke proved client/daemon negotiation and
SIGINT cleanup of the isolated socket. `namcap` reported zero errors; remaining
warnings are indirect runtime discovery (fonts/fontconfig/Wayland/xdg terminal)
or base-system dependencies.

The package itself is intentionally ignored rather than committed. Its size and
SHA-256 are recorded in `summary.json`; reproduce it from the recorded commit
with the build command above.
