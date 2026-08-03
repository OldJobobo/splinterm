# Private Arch prerelease packaging

Splinterm's `packaging/PKGBUILD` is a local validation artifact for the private
`0.1.0.pre` release. It is not an AUR publication recipe and does not upload
source or packages.

## One-command local installation

On an x86_64 Arch/Omarchy machine, a clean clone or pull can build and install
Splinterm with:

```bash
./install.sh
```

This installs missing build/runtime dependencies, builds the committed checkout,
validates the package contents, and asks before installation. It deliberately
does not change the default terminal or edit Omarchy configuration. It never
opts a fresh installation into the optional MCP package; when MCP is already
installed, the matching split package is upgraded to preserve its exact-version
dependency. Package tests are omitted from this installation path; pass
`--check` to run the complete `PKGBUILD` `check()` function. Pass `--yes` only
for an already-approved unattended installation.

## Build without installing

The package source must be an exact committed snapshot. The guarded build and
validation entry point is:

```bash
tools/package/build-local-package.sh
```

Pass `--no-check` to build and validate package contents without running the
complete package test suite. This is the mode used by `./install.sh` unless its
`--check` option is supplied.

Its equivalent manual build from a clean checkout is:

```bash
git archive --format=tar.gz --prefix=splinterm-0.1.0.pre/ \
  -o packaging/splinterm-0.1.0.pre.tar.gz HEAD
(
  cd packaging
  makepkg --cleanbuild --syncdeps --noconfirm
)
```

`makepkg` builds release binaries with the lockfile, runs workspace tests, and
creates the main package plus the explicitly optional `splinterm-mcp` split
package without installing either. Inspect them with:

```bash
pacman -Qlp packaging/splinterm-0.1.0.pre-1-x86_64.pkg.tar.zst
pacman -Qlp packaging/splinterm-mcp-0.1.0.pre-1-x86_64.pkg.tar.zst
namcap packaging/PKGBUILD packaging/*.pkg.tar.zst   # optional
```

The local archive checksum is intentionally `SKIP`: the archive is generated
from the reviewed local Git commit, is never downloaded, and the package's
`.BUILDINFO`/`.PKGINFO` record the actual build. Public distribution requires a
versioned immutable source URL and checksum.

## Installed layout

- `/usr/bin/splinterm`, `/usr/bin/splinterd`, the dedicated
  `/usr/bin/splinterm-relay` SSH transport, and the adjacent
  `/usr/bin/splinterm-pty-child` helper;
- optional split package `splinterm-mcp`, containing only the independently
  policy-authorized `/usr/bin/splinterm-mcp`, its setup guide, and notices;
- `/usr/bin/splinterm-xdg-terminal-exec`, its `splinterm-sessions` and
  `splinterm-reopen` session UX aliases, the public-CLI-only
  `/usr/bin/splinterm-session-picker` reference client, and the optional
  `/usr/bin/generate-omarchy-theme.py` JSON exporter;
- desktop entry, AppStream metadata, scalable icon, and user service;
- headless lifecycle/policy guidance, the exact terminal-image compatibility
  matrix, and integration snippets under `/usr/share/doc/splinterm/`; and
- MIT/project third-party notices under `/usr/share/licenses/splinterm/`.

The package does not modify a user's home, enable a service or lingering,
replace their terminal preference, edit SSH policy, or edit
`/usr/share/omarchy`. The desktop launcher starts `splinterd.service` on demand.
The headless unit reads an optional owner-controlled environment file, reloads
policy with SIGHUP, and stops with SIGINT so the daemon can reap its shells and
remove its socket cleanly. If protocol negotiation fails after an upgrade, it
restarts the user daemon once and waits a bounded 2.5 seconds. This ends old
daemon-owned shells because cross-version process migration is not promised.
See [headless.md](headless.md) for the complete service, policy, backup, and
recovery workflow, [remote.md](remote.md) for the policy-scoped SSH relay,
[integrations.md](integrations.md) for reference-client and in-Splint workflows,
and [mcp.md](mcp.md) for the optional adapter's host and digest-policy setup.

## Optional user integration

Splinterm follows the active Omarchy Quattro theme natively; no theme hook or
user-generated palette is required. To prefer Splinterm through
`xdg-terminal-exec`, prepend its desktop ID to the
user-owned preference file; do not overwrite existing entries:

```text
com.oldjobobo.splinterm.desktop
```

The reference snippet is `/usr/share/doc/splinterm/xdg-terminals.list`.

The default launcher always creates a fresh terminal. Use the **Recent
Sessions** desktop action or `splinterm-sessions` to choose an existing running
window, and **Reopen Last Session** or `splinterm-reopen` for the last locally
remembered window whose complete pane layout is still running. These aliases
also start the user daemon on demand. They never restore exited processes. See
`configuration.md` for the suggested Omarchy shortcut split.

## Live installation and rollback

Building and inspecting does not require installation. The guarded local
upgrade command validates the newest package under `packaging/`, shows the
installed and candidate versions, warns before ending daemon-owned shells,
installs with Pacman, reloads the user unit, and restores the daemon's previous
running state:

```bash
tools/package/upgrade-local-package.sh
```

To first build and validate a package from the clean, committed checkout:

```bash
tools/package/upgrade-local-package.sh --build
```

The command uses `sudo` because it is intended to run in an interactive
terminal, matching Omarchy's privilege convention. Pass `--yes` only for an
already-approved unattended invocation.

The equivalent manual lifecycle is:

```bash
systemctl --user stop splinterd.service
sudo pacman -U packaging/splinterm-0.1.0.pre-1-x86_64.pkg.tar.zst
systemctl --user daemon-reload
systemctl --user start splinterd.service
```

Install the adapter only when an MCP host will be configured:

```bash
sudo pacman -U packaging/splinterm-mcp-0.1.0.pre-1-x86_64.pkg.tar.zst
```

The guarded upgrade script upgrades `splinterm-mcp` only when that optional
package is already installed; it never opts a user in. Remove it independently
with `sudo pacman -Rns splinterm-mcp`. Remove the main package with
`sudo pacman -Rns splinterm`. User-owned config, explicit theme overrides,
policy, and durable state are deliberately not deleted by package scripts.
