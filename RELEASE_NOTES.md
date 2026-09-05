# Splinterm 0.1.0

Splinterm's first stable release is for x86_64 Omarchy/Arch Linux with native
Wayland under Hyprland. It carries the RC3 terminal and daemon implementation
forward unchanged; this release updates version metadata, documentation, and
publication tooling rather than adding product features.

## What ships

- Daemon-owned terminal sessions that survive closing a graphical Window,
  multiplexed Splints and Dojos, saved Lairs, and explicit restore.
- Scrollback and search, terminal images, native Wayland input, and configurable
  terminal lifetime and presets.
- Live Omarchy theme and default-font following. Invalid font generations retain
  the last valid renderer, while explicit font choices remain authoritative.
- Bounded JSON/NDJSON automation, remote graphical access, and an optional
  policy-scoped MCP adapter. Terminal output never grants automation authority.
- Source-built and prebuilt Arch packages, desktop integration, and a systemd
  user service.

## Stabilization included from RC3

- Unchanged Fontconfig sources no longer trigger repeated staging when an
  incompatible bold or italic face falls back to the regular face.
- FIFO policy files are rejected without waiting for a writer.
- Revoking another automation connection preserves partially received requests.
- Abnormal connection exits immediately remove their topology subscriptions.
- Relay cancellation interrupts blocked writes and reclaims queues. Remote EOF
  still delivers buffered bytes in order to slow consumers, within channel bounds.

The release also retains the RC1/RC2 font-lifetime fixes, current Gum colors for
new Splints, bounded Sixel previews for Yazi, and legacy generated-Dojo name
normalization.

## Install

Use `yay -S splinterm-bin` for the prebuilt package or `yay -S splinterm` to build
from source. The optional MCP packages are `splinterm-mcp-bin` and `splinterm-mcp`.
AUR distribution follows verification of the GitHub release assets. Packages and
source are also available on this release page.

## Upgrade boundary

**Splinterm 0.1 does not support live daemon upgrade handoff.** Stopping or
replacing the running daemon ends its child processes; saved topology is not a
checkpoint of running applications. Save your work and upgrade from Foot or
another terminal that is not owned by `splinterd`:

```bash
systemctl --user stop splinterd.service
# Upgrade the splinterm package here.
systemctl --user daemon-reload
systemctl --user start splinterd.service
```

Then reopen Splinterm Windows. Package installation does not silently restart
the user service. See the [upgrade and rollback documentation](https://splinterm.com/docs/packaging/).

## Scope and limitations

Stable 0.1.0 does not add support for other distributions, compositors,
architectures, or package formats, and does not promise live daemon replacement
or a support lifetime. Splinterm is security-conscious, not absolutely secure;
automation remains subject to explicit policy, consent, revocation, and resource
bounds. Future 0.x releases may change interfaces with documented migration.

[Documentation](https://splinterm.com/docs/) · [Source and issues](https://github.com/OldJobobo/splinterm)
