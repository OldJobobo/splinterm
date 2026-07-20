# Private Arch prerelease packaging

Splinterm's `packaging/PKGBUILD` is a local validation artifact for the private
`0.1.0.pre` release. It is not an AUR publication recipe and does not upload
source or packages.

## Build without installing

The package source must be an exact committed snapshot. The guarded build and
validation entry point is:

```bash
tools/package/build-local-package.sh
```

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
creates a package without installing it. Inspect it with:

```bash
pacman -Qlp packaging/splinterm-0.1.0.pre-1-x86_64.pkg.tar.zst
namcap packaging/PKGBUILD packaging/*.pkg.tar.zst   # optional
```

The local archive checksum is intentionally `SKIP`: the archive is generated
from the reviewed local Git commit, is never downloaded, and the package's
`.BUILDINFO`/`.PKGINFO` record the actual build. Public distribution requires a
versioned immutable source URL and checksum.

## Installed layout

- `/usr/bin/splinterm`, `/usr/bin/splinterd`, and the adjacent
  `/usr/bin/splinterm-pty-child` helper;
- `/usr/bin/splinterm-xdg-terminal-exec` and
  `/usr/bin/generate-omarchy-theme.py`;
- desktop entry, AppStream metadata, scalable icon, and user service;
- examples and integration snippets under `/usr/share/doc/splinterm/`; and
- MIT/project third-party notices under `/usr/share/licenses/splinterm/`.

The package does not modify a user's home, enable a service, replace their
terminal preference, or edit `/usr/share/omarchy`. The desktop launcher starts
`splinterd.service` on demand. The unit stops with SIGINT so the daemon can reap
its shell and remove its socket cleanly. If protocol negotiation fails after an
upgrade, it restarts the user daemon once and waits a bounded 2.5 seconds. This ends old
daemon-owned shells because cross-version process migration is not promised.

## Optional user integration

To apply Omarchy themes, copy the packaged hook once:

```bash
mkdir -p ~/.config/omarchy/hooks/theme-set.d
install -m755 /usr/share/doc/splinterm/omarchy/10-splinterm.sh \
  ~/.config/omarchy/hooks/theme-set.d/10-splinterm.sh
~/.config/omarchy/hooks/theme-set.d/10-splinterm.sh
```

To prefer Splinterm through `xdg-terminal-exec`, prepend its desktop ID to the
user-owned preference file; do not overwrite existing entries:

```text
com.oldjobobo.splinterm.desktop
```

The reference snippet is `/usr/share/doc/splinterm/xdg-terminals.list`.

## Live installation and rollback

Building and inspecting does not require installation. Install only after
explicit approval:

```bash
sudo pacman -U packaging/splinterm-0.1.0.pre-1-x86_64.pkg.tar.zst
```

Before upgrade/removal, close clients and stop the user daemon if its shells are
no longer needed:

```bash
systemctl --user stop splinterd.service
```

Remove with `sudo pacman -Rns splinterm`. User-owned config, theme hooks, and
runtime state are deliberately not deleted by package scripts.
