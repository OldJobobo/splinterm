# Splinterm 0.1.1-rc.3

Third release candidate for Splinterm 0.1.1, intended as a GitHub prerelease
for testing before the final release. This is not the stable 0.1.1 release.
AUR packages remain on the stable release; no RC packages are published to AUR.

## Changes since RC2

- Scrollback search paints its query and match status inside the Window canvas,
  including when the Explorer is open. The field stays on the terminal side of
  the Explorer drawer and is omitted if there is no room to show it.
- New Splints, Dojos, and Lairs created within a Window inherit the focused
  Splint's live root-process working directory when available, including remote
  sessions. Explicit working directories still take precedence.
- Locally closed graphical relay channels discard valid late responses without
  failing unrelated channels. Scrollback-tail merges avoid repeated front
  shifts while retaining the existing bounded history behavior.
- Explorer activity filters, context-action reconciliation, and context-menu
  sizing received maintenance fixes after RC2.

## Included from RC2

- Explorer right-click menus act on the clicked Lair, Dojo, or Splint without
  implicitly navigating to it. Applicable actions include rename, creation,
  save/pin/restore, focus/split/close, and confirmed termination. Targets are
  revalidated before dispatch and confirmation.
- Generated navigation names use short display labels such as `Lair 1`,
  `Dojo 1`, and `Terminal 1`. Custom names and underlying identities remain
  unchanged. Display numbering may change as siblings are added or removed.
- Explorer names and status text occupy separate lines instead of competing
  for row width.
- A rejected tab context-menu request no longer exits the whole client when
  another input operation or transition makes the menu unavailable.

## Navigation and returning to work

- An optional Lair Explorer presents Lairs, Dojos, and Splints beside terminal
  panes. Use **Toggle Lair explorer** or **Focus Lair explorer** in the command
  palette. Selection and current terminal focus remain distinct.
- Hierarchy-aware pickers and activation/focus recovery make it easier to return
  to running work and reattach restored panes.
- The first-five-minutes guide covers opening, splitting, detaching, and returning.
  Reattachment does not restart an exited process; explicit restore starts new
  processes rather than recovering application checkpoints.
- Split-below and split-right now honor their named directions in both keyboard
  and palette dispatch. Default keys and action IDs are unchanged.

## Text and clipboard

- Opt-in `main.font-ligatures=on` or `cursor` shapes compatible adjacent printable
  ASCII cells. The default remains `off`; cell widths and copied text are
  unchanged. This is not general-script or bidi run shaping.
- `main.font-features` accepts explicit OpenType feature settings. Both settings
  require a new client process and survive native font-family changes unchanged.
- **Save clipboard image and insert path** saves bounded static PNG content into
  a configured private directory and inserts its shell-quoted path without Enter.
  It is local-only, has no default shortcut, and does not change ordinary paste.
  Saved files persist until manually deleted; this is not metadata sanitization.

## Accessibility and remote identity

- Native Linux AT-SPI exposes Explorer navigation, search, and bounded status.
  Actions respect focus, control, visibility, and stale-target checks. Terminal
  text, scrollback, modal dialogs, and global pointer geometry are not exposed.
  This is not whole-terminal screen-reader support.
- The accessibility guide is included in the Arch and Nix package documentation.
- Remote Windows show the connected daemon's hostname when available, with a
  `Remote` fallback. This label is informational, not proof of connection health
  or a security indicator.

## Packaging and reliability

- The repository flake and NixOS module provide x86_64 Linux packaging, including
  headless/module/package checks. Nix support predates this release in maintenance
  ancestry but is absent from the original `v0.1.0` tag. The Arch release installer
  is not a NixOS installer; universal compositor compatibility is not claimed.
- Fixes address inactive-pane repainting, committed-canvas preservation, control
  reacquisition, subscription progress, modal input precedence, and Explorer
  selection clipping.
- Switching to light Omarchy themes now honors Foot's `initial-color-theme=light`
  and loads the light palette instead of retaining the previous dark background.
  Dark/legacy palettes and explicit alpha/blur overrides remain supported.
- History caches retain prefixes only across proven continuous updates, avoiding
  disconnected blank prefixes after bounded scrollback-tail updates. The wire
  history cap is unchanged.
- Arch renderer checks use checksum-pinned font fixtures, not ambient primary-font
  selection. These fixtures are not installed as runtime fonts. MCP validation
  now explicitly synchronizes asynchronous catalogue and cancellation boundaries.

## Installation and upgrade boundary

Once published, use the packages attached to the GitHub `v0.1.1-rc.3` prerelease for RC testing;
verify them against its published checksums before installation. Install the
matching optional MCP package if needed. AUR remains on the stable release until
0.1.1 final is approved; do not use the repository's candidate AUR templates as
published package recipes. Final publication requires a separate readiness
decision after RC testing.

**There is no live daemon-upgrade handoff.** Save work and upgrade from Foot,
another independent terminal, or an independent SSH session. Stopping or replacing
`splinterd` ends its child processes; saved layouts do not checkpoint applications.
Reopen Splinterm Windows after replacement so they use the new packaged client
identity. Keep the optional MCP adapter at the matching package version.

See `docs/packaging.md`, `docs/nixos.md`, and `docs/accessibility.md` for platform,
upgrade, and support boundaries. No broad performance guarantee, universal
assistive-technology compatibility, or support-duration promise is introduced.
