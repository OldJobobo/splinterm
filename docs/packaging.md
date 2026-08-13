# Public alpha Arch packaging

Release authority, candidate construction, approval boundaries, and the future
n8n notification role are defined in [Release automation](release-automation.md).

Splinterm's `packaging/PKGBUILD` produces the public alpha `0.1.0alpha2` split
package for reviewed local and CI builds. Its local source archive and `SKIP`
checksum are valid only in that workflow. The source-built AUR authority is
`packaging/aur/PKGBUILD`, which uses the immutable `v0.1.0-alpha2` release
asset and a reviewed SHA-256 checksum. It intentionally omits a `check()` phase.
The recommended prebuilt authority is `packaging/aur-bin/PKGBUILD`; it repackages
checksummed artifacts from a successful immutable edge release without compiling
or testing on the user's machine.

## Versioned AUR installation

Install the recommended prebuilt main package and optional exact-version MCP
adapter with an AUR helper:

```bash
yay -S splinterm-bin
yay -S splinterm-mcp-bin
```

The source-built alternatives are `splinterm` and `splinterm-mcp`. Migrating to
the `-bin` packages prompts once to replace those conflicting source packages.
`paru` may be used instead of `yay`. AUR availability does not expand the
validated target beyond x86_64 Arch/Omarchy with native Wayland/Hyprland or add
a stable compatibility and support-duration promise.

## One-command prebuilt edge installation

On an x86_64 Arch/Omarchy machine, a clone installs or updates to the newest
successfully built `main` commit without compiling locally:

```bash
./install.sh
```

`.github/workflows/edge-release.yml` performs a clean release build and validates
both extracted packages in an Arch container; the separate CI workflow retains
the full workspace source-test suite. Package assets include the full Git commit
in their names and
are published under the immutable `edge-<commit>` release. Only after that
release is complete does the workflow atomically force-update the
`edge-channel` Git ref to a one-file commit containing `edge-manifest.json`.
That manifest binds the repository, architecture, release, commit, exact asset
names, and SHA-256 digests. An interrupted publication therefore leaves either
the prior channel commit or the complete new one rather than exposing a mixed
package set.

The installer uses an authenticated GitHub CLI session when one is already
available and otherwise downloads the public channel manifest and release assets
with `curl`. Authentication is not required for ordinary public alpha installs.

Before Pacman installation, the script validates the closed manifest shape,
commit-bound release and asset names, checksums, architecture, and matching
split-package versions. It rejects a shadowing user-local client, preserves an
emergency snapshot of replaced binaries for diagnosis or manual recovery, warns
before stopping daemon-owned shells, restores the daemon after failure, and
checks Pacman integrity, the desktop entry, and trusted-client sibling identity
after restart. The snapshot is not presented as a Pacman package rollback;
reinstall a previously retained package for a package-consistent downgrade. It deliberately does not change the default terminal or
edit Omarchy configuration. A fresh installation does not install the optional
MCP package; an existing MCP installation is upgraded to preserve its
exact-version dependency. Pass `--yes` only for an already-approved unattended
installation.

## One-command source installation

To compile and install the current committed checkout instead, use an external
terminal such as Foot—not a shell running inside Splinterm:

```bash
./install.sh --source
```

Both source and prebuilt installation refuse before service or package work when
the installer has a `splinterd` or `splinterm-pty-child` ancestor. Stopping the
daemon from one of its own shells would terminate the installer before Pacman or
its recovery path could complete.

This installs missing build/runtime dependencies, builds the committed checkout,
validates package contents, and asks before installation. Pass `--check` (which
implies `--source`) to run the complete `PKGBUILD` `check()` function.

## Build without installing

The package source must be an exact committed snapshot. The guarded build and
validation entry point is:

```bash
tools/package/build-local-package.sh
```

Pass `--no-check` to build and validate package contents without running the
complete package test suite. This is the mode used by `./install.sh --source`;
`./install.sh --check` retains the complete package test suite.

Its equivalent manual build from a clean checkout is:

```bash
git archive --format=tar.gz --prefix=splinterm-0.1.0alpha2/ \
  -o packaging/splinterm-0.1.0alpha2.tar.gz HEAD
```

The archive honors `.gitattributes` `export-ignore` entries; website source and
website deployment automation are intentionally excluded from package release
inputs. Continue the manual equivalent with:

```bash
(
  cd packaging
  makepkg --cleanbuild --syncdeps --noconfirm
)
```

`makepkg` builds release binaries with the lockfile, runs workspace tests, and
creates the main package plus the explicitly optional `splinterm-mcp` split
package without installing either. Inspect them with:

```bash
pacman -Qlp packaging/splinterm-0.1.0alpha2-1-x86_64.pkg.tar.zst
pacman -Qlp packaging/splinterm-mcp-0.1.0alpha2-1-x86_64.pkg.tar.zst
namcap packaging/PKGBUILD packaging/*.pkg.tar.zst   # optional
```

The local archive checksum is intentionally `SKIP`: the archive is generated
from the reviewed local Git commit, is never downloaded, and the package's
`.BUILDINFO`/`.PKGINFO` record the actual build. The separate checked-in AUR
recipe uses the immutable versioned release asset and real SHA-256 checksums; do
not submit the local-build recipe to the AUR.

## Installed layout

- `/usr/bin/splinterm`, `/usr/bin/splinterd`, the dedicated adjacent
  `/usr/bin/splinterm-relay` SSH transport (byte-transparent `--stdio` and
  bounded `--graphical-stdio` modes), and `/usr/bin/splinterm-pty-child`;
- optional split package `splinterm-mcp`, containing only the independently
  policy-authorized `/usr/bin/splinterm-mcp`, its setup guide, and notices;
- `/usr/bin/splinterm-xdg-terminal-exec`, which preserves structured XDG argv
  and selects persistent commandless or transient command-bearing launches, its
  canonical `splinterm-dojos` and `splinterm-reopen` Dojo UX aliases, the
  compatibility `splinterm-sessions` alias, the public-CLI-only
  `/usr/bin/splinterm-dojo-picker` reference client and compatibility
  `/usr/bin/splinterm-session-picker` alias, and the optional
  `/usr/bin/generate-omarchy-theme.py` JSON exporter;
- desktop entry, AppStream metadata, scalable icon, and user service;
- the release README, CLI/usage/configuration guides, built-in Omarchy keymap
  documentation, preset guide and example schema, optional shell-integration
  instructions, headless lifecycle/policy guidance, terminal-image matrix, and
  integration snippets under `/usr/share/doc/splinterm/`; and
- MIT/project third-party notices under `/usr/share/licenses/splinterm/`.

The package does not modify a user's home, enable a service or lingering,
replace their terminal preference, edit SSH policy, or edit
`/usr/share/omarchy`. The desktop launcher starts `splinterd.service` on demand.
The headless unit reads an optional owner-controlled environment file, reloads
policy with SIGHUP, and stops with SIGINT so the daemon can reap its shells and
remove its socket cleanly. If protocol negotiation fails after an upgrade, it
restarts the user daemon once and waits a bounded 2.5 seconds. This ends old
daemon-owned shells because cross-version process migration is not promised.
The local and remote packages must be version-compatible, and the relay must
remain adjacent to the exact packaged `splinterd` executable because every
logical graphical channel repeats executable-identity validation. See
[headless.md](headless.md) for the complete service, policy, backup, and
recovery workflow, [remote.md](remote.md) for both policy-scoped SSH relay modes,
[integrations.md](integrations.md) for reference-client and in-Splint workflows,
and [mcp.md](mcp.md) for the optional adapter's host and digest-policy setup.

## Optional user integration

Splinterm follows the active Omarchy Quattro theme natively; no theme hook or
user-generated palette is required. The explicit unified integration makes
Splinterm the current user's XDG default terminal, gives its Window Omarchy's
`terminal` tag, and enables the guarded screensaver adapter:

```bash
splinterm integration omarchy enable
splinterm integration omarchy status
```

`disable` removes only exact managed objects and restores the previous
`xdg-terminals.list` object byte-for-byte, including a relative or dangling
symlink. Components already configured externally are reported as ready but are
never claimed or removed. A versioned state journal under
`${XDG_STATE_HOME:-~/.local/state}/splinterm/integrations/` prevents guessing
after an interrupted transaction.

Package installation still changes none of these user files. The reference
terminal-list snippet remains `/usr/share/doc/splinterm/xdg-terminals.list` for
manual setups.

The default launcher always creates a fresh terminal. Use the **Recent Dojos**
desktop action or `splinterm-dojos` to choose an existing running Dojo, and
**Reopen Last Dojo** or `splinterm-reopen` for the last locally remembered Dojo
whose complete pane layout is still running. `splinterm-sessions` remains a
compatibility alias. These aliases
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

Trusted graphical authority requires `/usr/bin/splinterm` to match the device
and inode of the client sibling adjacent to the running `/usr/bin/splinterd`.
After an upgrade replaces that sibling, close and reopen every existing
Splinterm Window: an already-running client retains the old inode and will be
rejected as unauthorized. Verify the new daemon/client sibling identity before
launching graphical acceptance; graphical testing remains a separately approved,
guarded operation.

The equivalent manual lifecycle is:

```bash
systemctl --user stop splinterd.service
sudo pacman -U packaging/splinterm-0.1.0alpha2-1-x86_64.pkg.tar.zst
systemctl --user daemon-reload
systemctl --user start splinterd.service
```

Install the adapter only when an MCP host will be configured:

```bash
sudo pacman -U packaging/splinterm-mcp-0.1.0alpha2-1-x86_64.pkg.tar.zst
```

The guarded upgrade script upgrades `splinterm-mcp` only when that optional
package is already installed; it never opts a user in. Remove it independently
with `sudo pacman -Rns splinterm-mcp`. Remove the main package with
`sudo pacman -Rns splinterm`. User-owned config, explicit theme overrides,
policy, and durable state are deliberately not deleted by package scripts.
