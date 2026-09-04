# Unreleased

- Native Omarchy font following is now live when `main.font` is unset. Valid
  fontconfig `monospace` family changes replace one complete immutable renderer
  generation without restarting the Window, daemon, shell, or applications.
- Explicit font patterns remain authoritative. Invalid live generations retain
  the last valid family; the documented JetBrains fallback is startup-only.
- Family changes preserve configured size/policy, padding, DPI, runtime zoom,
  topology, focus, history, modal and IME state, and controller authority.
  Observer panes never acquire control solely to resize after a font change;
  deferred resizes retry and ambient probes use a bounded ten-second cadence.
- Fontconfig named variable-font instances survive shaping and rasterization.
  Accepted staging remains synchronized with watcher probes, and cached
  FreeType faces share immutable font mappings across raster sizes.
- Non-graphical implementation checks are recorded; packaged graphical
  acceptance and publication remain separate unreleased gates.

# Splinterm 0.1.0 Beta 1 — The Foundation Holds

Splinterm has reached public beta.

That is more than a version-label change. The alpha releases proved that the
core idea could work: a native terminal whose shells, layouts, and state belong
to a persistent daemon instead of a window. Beta 1 is the point where that idea
has become a coherent product we are comfortable asking people to use, test,
and build workflows around.

Close a window attached to a persistent Splinterm session and your work keeps
running. Come back through the session picker (the Dojo picker) or
`splinterm reopen`, and it is still there. Tabs and panes are views into
persistent sessions rather than fragile containers for them. Command-hosting
XDG launches are deliberately transient and end with their owner window. The
same topology is available to people, remote clients, scripts, and the optional
MCP adapter—without giving automation the authority of a human at the keyboard.

## What is ready in Beta 1

- **Persistent terminal sessions.** Shells and processes in persistent sessions
  survive graphical client disconnects, with explicit restore behavior when a
  process has exited. Command-hosting XDG launches remain intentionally
  transient.
- **A native Wayland workflow.** Splinterm provides windows, panes, window-local
  Dojo tabs, search, copy mode, clipboard integration, IME support, and
  multi-client control on the validated Omarchy/Arch Linux target.
- **First-class Omarchy integration.** Splinterm follows the active Omarchy
  palette, can reload valid theme changes without restarting your shell, and
  offers an explicit, reversible default-terminal integration.
- **One terminal world for humans and tools.** The human CLI, structured
  JSON/NDJSON clients, SSH relay, native remote client, and optional MCP adapter
  all work with the same persistent sessions.
- **Bounded automation.** Machine access uses explicit policy, scopes, resource
  limits, controller ownership, consent, and revocation. Terminal output remains
  untrusted data; it cannot grant itself authority.
- **Real packages.** Immutable GitHub release assets and both prebuilt and
  source-built AUR packages are now public and versioned.

## What changed for Beta 1

### Large terminals no longer hit the old grid ceiling

Splinterm now supports terminal grids up to `480×128`, replacing the earlier
`240×80` ceiling. Maximized terminals on validated 1440p and non-graphically
verified 4K profiles can use their available cell area instead of silently
stopping at an inherited protocol limit. Negotiated limits, oversized terminal
transactions, renderer state, and publication memory all remain bounded.

### Heavy output is steadier and more predictable

Terminal updates now travel through sparse publication frames that own the rows
and metadata that actually changed rather than repeatedly retaining complete
terminal checkpoints. This reduces unnecessary retained state and large
cross-batch materialization while preserving exact ordering, resynchronization,
exit delivery, and hard memory ceilings.

In practical terms: sustained and bursty terminal output has a much healthier
path through the daemon and graphical client, without weakening correctness to
win a benchmark.

### Active tabs are readable without corrupting selection colors

On native Omarchy themes, the active tab now derives its background from
standard theme roles and chooses a high-contrast foreground independently from
terminal selection colors. Theme authors do not need Splinterm-specific keys,
terminal text selection keeps its intended palette, and valid live theme
changes remain atomic.

### Wide-grid and automation edge cases were hardened

The session picker now accepts the full Beta 1 grid envelope, constrained
endpoints retain their negotiated dimensions, and a control-subscription
ordering race found by repeated CI was fixed before release. Release review also
caught user-facing package metadata defects, so the candidate was corrected and
rebuilt before promotion.

That matters. Beta does not mean “finished.” It means the release boundary is
strong enough to catch problems before users inherit them.

## Install

Splinterm Beta 1 currently targets **x86_64 Omarchy/Arch Linux on native
Wayland**. The recommended prebuilt packages are:

```bash
yay -S splinterm-bin
# Optional policy-scoped MCP adapter:
yay -S splinterm-mcp-bin
```

Source-built packages are available as `splinterm` and `splinterm-mcp`. See the
[installation guide](https://splinterm.com/docs/install/) for trusted-client
identity, upgrades, integration, and troubleshooting.

## What “beta” means here

The persistent-session model, terminal core, native presentation, multiplexing,
packaging, and bounded automation workflows are implemented and validated for
the documented target. This is the first Splinterm release intended for serious
public evaluation rather than early architectural proof.

It is still beta software. Interfaces may change. The supported environment is
narrow. Broader compositor, distribution, architecture, and package support is
not promised yet, and stable compatibility or support windows have not been
announced. The renderer is currently CPU-composed Wayland shared memory, remote
image transfer is not supported, and full Kitty graphics compatibility is not
claimed.

Those boundaries are deliberate. Splinterm would rather make a small promise it
can defend than a large one it cannot.

## From here

Beta 1 gives Splinterm a solid public floor: persistent by design, native where
it matters, useful to humans, accessible to tools, and explicit about authority.
The next phase is refinement—better everyday ergonomics, broader compatibility,
and a careful path toward stable interfaces—without sacrificing the ownership
and safety model that made Splinterm worth building in the first place.

Thank you to everyone willing to install it, stress it, report what feels wrong,
and help shape what comes next.

- **Release:** [`v0.1.0-beta1`](https://github.com/OldJobobo/splinterm/releases/tag/v0.1.0-beta1)
- **Current status:** [`docs/status.md`](https://raw.githubusercontent.com/OldJobobo/splinterm/main/docs/status.md)
- **Documentation:** [splinterm.com/docs](https://splinterm.com/docs/)
