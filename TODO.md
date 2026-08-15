# TODO

## Beta 1 active-tab foreground contrast patch

- [ ] Complete [Plan 0041](docs/plans/0041-beta1-active-tab-foreground.md).
- [x] Add optional `active_tab_foreground` roles to native Omarchy and strict
  JSON theme resolution.
- [x] Keep terminal `selection_foreground` independent from active Dojo-tab text.
- [x] Fall back deterministically to the theme background or foreground with
  higher contrast against the resolved active-tab background.
- [x] Prove Dispatch resolves `#141d23` text on `#e6c93a` without changing its
  terminal selection colors.
- [x] Pass focused and serial non-graphical validation plus fresh read-only
  review.
- [ ] Complete separately approved packaged graphical acceptance before release.

Version integration, candidate construction, publication, and AUR distribution
remain separate release-boundary work.

## Alpha3.2 Backspace crash hotfix

- [x] Keep held Backspace and other ordinary terminal input nonblocking when a
  pane controller's bounded command queue is temporarily saturated.
- [x] Bound pending terminal input globally, preserve pane identity and complete
  input units, and retain input-before-focus/control ordering.
- [x] Route file-drop input through the same bounded path and discard stale
  pane-bound batches safely after pane or tab teardown.
- [x] Add saturation, multi-pane, ordering, atomicity, retry, and teardown
  regressions; pass full release CI and independent review.
- [x] Publish `v0.1.0-alpha3.2` and both `0.1.0alpha3.2-1` AUR package bases.

## Alpha3.1 transient-window hotfix

- [x] Hide the tab strip by default when XDG invokes Splinterm with a
  command-bearing `-e` launch; these transient command windows should not present
  ordinary persistent-Dojo tabs initially.
- [x] Keep transient XDG Lairs excluded from persistence while allowing their
  live owner Window to create and attach additional Dojos and splits.
- [x] Give selected Dojo tabs an exact theme-provided background role and apply
  the configured terminal alpha to both the strip and selected-tab body.

## Alpha3 command palette and keymaps

- [x] Complete [Plan 0033](docs/plans/0033-alpha3-command-palette-and-keymap-closure.md).
- [x] Make `dojo.close-other-tabs` bindable and route keyboard activation through
  the same Window-local, non-destructive behavior as palette activation.
- [x] Add the bounded everyday palette commands for binding help, configuration
  reload, copy mode, pane zoom, Dojo and Lair navigation/management, and Window
  detachment.
- [x] Add drift invariants across palette commands, bindable actions, built-in
  profiles, shortcut labels, availability, and typed runtime dispatch.
- [x] Share terminal `Super+C/V` safely across `splinterm` and `omarchy-tmux`,
  accept Omarchy's terminal-tagged `Ctrl+Insert`/`Shift+Insert` translation,
  retain owned-field `Super+C/V/X/Z` in both, define copy-mode behavior, and preserve
  terminal-pane `Super+X/Z` passthrough.
- [x] Prove every built-in `splinterm` and `omarchy-tmux` binding and a
  representative strict custom overlay through non-graphical end-to-end tests.
- [x] Reconcile usage, configuration, PRD, status, and fixed command-count claims.
- [x] Perform separately approved packaged graphical acceptance for the expanded
  palette, resolved help/labels, reload, both built-in profiles, and
  close-other-tabs semantics.

Arbitrary preset/shell command bindings, plugin-defined trusted commands,
numeric Dojo-selection rows, raw send-prefix, and broad palette redesign remain
outside the alpha3 slice.

## Alpha3 scrollback Enter safety

- [x] Complete [Plan 0035](docs/plans/0035-alpha3-scrollback-enter-safety.md).
- [x] Make Return and keypad Enter on a historical focused Splint use the
  existing Return-to-Live path and send zero PTY bytes.
- [x] Consume the initiating physical key through release so repeat events cannot
  submit input after the viewport becomes live.
- [x] Preserve normal Enter behavior when already live and preserve all trusted
  modal Enter precedence.
- [x] Add focused press/repeat/release, multi-pane/tab, modal-isolation, redraw,
  and PTY-input regressions.
- [x] Perform separately approved packaged graphical proof that historical Enter
  returns live without executing a prepared command and a second live Enter
  submits exactly once.

## Dojo picker vocabulary

- [x] Make `splinterm dojos` the canonical picker command.
- [x] Rename user-facing **Session Picker** and **Recent Sessions** surfaces to
  **Dojo Picker** and **Recent Dojos**.
- [x] Add canonical `splinterm-dojos` and `splinterm-dojo-picker` executables.
- [x] Retain `splinterm sessions`, `splinterm-sessions`, and
  `splinterm-session-picker` as compatibility aliases.
- [x] Update current desktop metadata, documentation, packaging, and validation.
- [x] Commit and publish the vocabulary changes in the versioned alpha and edge
  packages.
- [x] Install the current packaged alpha and perform guarded graphical acceptance
  of the picker labels; canonical commands and compatibility aliases remain
  covered by package and command-surface validation.

Private `SessionPicker*` identifiers may remain until a broader internal rename
is justified; they do not define user-facing product vocabulary.

## Lair retention and saved-workspace lifecycle

- [x] Complete [Plan 0034](docs/plans/0034-alpha3-saved-lair-layouts.md) for
  `0.1.0-alpha3`.
- [x] Define explicit Live, Detached, Saved, Restorable, Pinned, and Disposable
  Lair states, retaining preset-derived provenance when known.
- [x] Add exact-target Save Layout, Pin/Unpin, Preview, Restore Lair, and Restore
  Dojo actions; confirmed metadata deletion retains the existing exact captured
  Lair termination path.
- [x] Preserve every saved Dojo tree, branch axis and ratio, default focus,
  Splint name, known structured launch argv, launch working directory, shell
  policy, and bounded rows/columns hints.
- [x] Classify explicit preset/command leaves as restorable applications and
  ordinary shell leaves as Shell; never infer an interactive foreground
  application from `/proc`, titles, prompts, or terminal output.
- [x] Show a body-free restore preview with tree shape, proportional sizes,
  working directories, application/shell classification, process count, and
  explicit non-restored state.
- [x] Never execute saved commands on daemon startup, login, package upgrade,
  save, pin, preview, or picker display; multi-process restore requires explicit
  confirmation.
- [x] Automatically retire only eligible fully exited Disposable Lairs under a
  bounded, documented policy; never retire Live, Detached, Saved, or Pinned
  Lairs automatically.
- [x] Preserve privacy: do not persist process memory, shell state, editor state,
  terminal or scrollback bodies, clipboard data, terminal input, environment
  values, secrets, or image bodies.
- [x] Add schema migration, round-trip, lifecycle, capacity,
  persistence-failure, partial-launch-failure, concurrency, and picker tests.
- [x] Perform separately approved packaged graphical save/preview/restore
  acceptance with unequal nested splits and mixed application/shell leaves.
- [x] Document retention defaults, proportional size restoration, destructive
  actions, explicit execution, limitations, and recovery behavior.

The existing durable topology already stores tree shape, ratios, launch metadata,
and bounded geometry hints. Plan 0034 productizes that foundation; it does not
promise live-process checkpointing or arbitrary foreground-application replay.

## Alpha3 Wayland file-drop path insertion

- [x] Complete the non-graphical implementation in [Plan 0036](docs/plans/0036-alpha3-wayland-file-drop-path-insertion.md).
- [x] Accept bounded `text/uri-list` drops with Wayland copy semantics and only
  local regular-file URIs.
- [x] Capture and revalidate the exact pane, Splint incarnation, tab, controller,
  and input generation; never retarget an asynchronous drop.
- [x] Insert one deterministic, space-separated POSIX-shell-escaped payload with
  no trailing space or submission bytes.
- [x] Preserve bracketed paste, modal isolation, all-or-nothing multi-file
  behavior, bounded feedback, and body-free diagnostics.
- [x] Test spaces, apostrophes, Unicode, leading dashes, multiple files, LF/CRLF,
  malformed encodings, remote hosts, stale targets, limits, and cancellation.
- [x] Perform separately approved packaged graphical acceptance without moving,
  opening, uploading, retaining, or executing dropped files.

Clipboard-image saving, directories, remote transfer, private file-manager MIME
types, and drag-out remain outside Alpha3.

## Post-alpha3 clipboard-image path insertion

- [ ] Define accepted clipboard image formats, destination, collision-safe naming,
  permissions, cleanup, confirmation, privacy, and failure behavior.
- [ ] Save accepted image bytes only after explicit user action, then insert the
  shell-escaped saved path without changing normal text paste.
- [ ] Test supported and unsupported formats, cancelled and failed saves,
  collisions, size limits, cleanup, and bracketed paste.

## Post-alpha3 user-customizable tabs

Target a scoped update after `0.1.0-alpha3` and before supported `1.0`:

- [ ] Write a focused plan for user-defined tab identity, behavior, and
  appearance.
- [ ] Support presentation-only custom labels, bounded icons, and pinned/favorite
  state without silently renaming persistent Dojos.
- [ ] Support deterministic ordering rules, initial active-tab policy, strip
  visibility policy, and user-selected default tab actions.
- [ ] Support per-tab shortcuts only for closed typed Splinterm actions, with
  strict conflict detection and generated help/shortcut labels.
- [ ] Support active/inactive colors, dimensions, padding, separators,
  alignment, icon/label spacing, close indicators, and bounded global/per-tab or
  per-Dojo overrides.
- [ ] Define Window-local versus reusable/durable customization ownership,
  schema precedence, migration, icon-source safety, and theme inheritance.
- [ ] Preserve accessibility, exact targeting, non-destructive detach,
  confirmation, modal isolation, pointer targets, compact layouts, fractional
  scaling, and 1–32-tab overflow behavior.
- [ ] Reject arbitrary CSS, shell commands, executable icon providers,
  terminal-controlled chrome, and externally registered trusted actions.
- [ ] Add schema, precedence, migration, conflict, renderer, input, overflow,
  scale, theme, and separately approved packaged graphical tests.

This milestone is not an alpha3 blocker, but it is planned before `1.0`.

## Theme palette fidelity

- [x] Preserve theme-provided selection RGB roles verbatim; themes own hue and
  selection presentation uses the exact resolved background role.
- [x] Replace the selection `blend_rect` path with an opaque exact-theme
  background layer rather than baking a derived RGB color into the framebuffer.
- [x] Keep selection composition from tinting already-rendered glyph colors;
  repaint selected glyphs and decorations as a separate foreground layer.
- [x] Add renderer tests proving exact resolved theme roles, including the Sakura
  Mochi `#f23888` selection color and the themed scrollback overlay.
- [x] Give selected Dojo tabs a dedicated exact theme-provided background role
  without mixing or deriving colors, inherit the configured terminal alpha for
  the strip and selected-tab body, and retain `ui_accent` as the contrasting
  opaque selected-tab underline.

## Omarchy integration

- [x] Implement Plan 0032's XDG-only app-ID transport, owned profile, packaged
  launcher helper, and explicit collision-safe activation workflow.
- [x] Rerun [Plan 0032](docs/plans/0032-omarchy-screensaver-integration.md)
  non-graphical validation on the coherent Alpha3 release state and inspect the
  extracted package.
- [x] Perform separately approved guarded packaged graphical acceptance for the
  Splinterm-owned, opt-in Omarchy screensaver integration.

## 0.2.0 persistence expansion and upgrade handoff

- [ ] Complete [Plan 0037](docs/plans/0037-0.2-persistence-and-upgrade-handoff.md).
- [ ] Approve precise persistence lifetime vocabulary, the in-place re-exec ADR,
  and a separate privacy decision before any durable terminal archive work.
- [ ] Prove an adoptable Linux PTY session preserves PID, process group, session,
  one-reader ownership, reaping, signaling, ordered I/O, and bounded rollback.
- [ ] Define bounded terminal checkpoint, descriptor, handoff, and rollback ABIs
  with exact parser continuation, corruption rejection, and migration tests.
- [ ] Add a daemon handoff coordinator that fences authority, adopts atomically,
  reconnects clients by full resnapshot, and fault-injects every rollback edge.
- [ ] Make launcher and packaging UX distinguish matching, compatible, blocked,
  bootstrap, destructive-fallback, downgrade, and interrupted upgrade states.
- [ ] Keep recipe-only reboot restore as the default; gate optional archives on
  reviewed retention, deletion, export, trusted-read, and privacy policy.
- [ ] Record serial workspace, package, identity, rollback, architecture,
  security/privacy, release, and separately approved graphical evidence.

## 0.2.0 live Omarchy system-font synchronization

- [ ] Complete [Plan 0038](docs/plans/0038-0.2-live-omarchy-font-sync.md).
- [ ] Follow Omarchy's fontconfig `monospace` family live only when
  `main.font` is unset; preserve explicit Splinterm font authority.
- [ ] Stage and publish complete renderer font generations transactionally,
  retaining the last valid generation when resolution or validation fails.
- [ ] Rebuild font-derived caches and Window geometry coherently while
  preserving topology, focus, history, output DPI, configured size/policy,
  padding, and runtime zoom.
- [ ] Emit at most one final PTY resize per affected live Splint and never
  restart the daemon, shell, or Window for a valid family change.
- [ ] Add precedence, fingerprint, coalescing, rollback, cache-retirement,
  geometry, resize-count, performance, and separately approved graphical tests.

## 0.2.0 searchable keybindings and Lair controls

- [ ] Complete [Plan 0039](docs/plans/0039-0.2-searchable-keybindings-and-lair-controls.md).
- [ ] Add bounded deterministic fuzzy search to the effective keybinding-help
  display across action labels, configuration names, shortcuts, sources, and
  closed keywords.
- [ ] Keep search editing modal and terminal-safe, with stable ranking,
  clear-then-close Escape behavior, bounded navigation, and a calm no-match state.
- [ ] Make Save, pin-toggle, Preview, and Restore current Lair closed bindable
  actions using the existing lifecycle targeting, availability, and confirmation
  paths.
- [ ] Add reviewed collision-free `omarchy-tmux` defaults while preserving every
  existing Lair shortcut and strict custom-overlay support.
- [ ] Prove registry, help, CLI inspection, command-palette shortcut projection,
  keyboard dispatch, and lifecycle behavior cannot drift.

## Benchmark coverage follow-up

WezTerm separates its mux-owned PTY, terminal, scrollback, pane, tab, window, and
workspace state from GUI presentation, but its default local mux may be embedded
in the GUI while `wezterm-mux-server` makes detachment a separate optional mode.
That differs from Splinterm's mandatory `splinterd` authority and disposable
Wayland client. Comparisons must therefore report complete process-group totals
and per-process attribution, distinguish embedded-local and detached Unix-domain
modes, and disclose that WezTerm's cross-platform GPU renderer and Lua extension
surface are not direct equivalents to Splinterm's Wayland-focused bounded
protocol and daemon policy model. Architecture references:
[Multiplexing](https://wezterm.org/multiplexing.html),
[mux API](https://wezterm.org/config/lua/wezterm.mux/index.html), and
[GUI API](https://wezterm.org/config/lua/wezterm.gui/index.html).

- [ ] Add WezTerm as an optional comparison terminal throughout the benchmark
  suite: provide isolated deterministic profiles and app IDs for applicable mux
  modes, extend terminal inventories and result schemas, include it in retention,
  latency, output, resize, lifecycle, idle, and scrollback matrices, preserve
  guarded workspace/focus/cleanup checks, and report a clear skip when WezTerm is
  not installed.

## Maintainability and architecture follow-up

Track the findings from the
[2026-08-14 code-size and architecture review](docs/reviews/2026-08-14-code-size-and-architecture.md).
These are changeability improvements, not evidence of a blocker-level system
redesign or a mandate to reduce security, protocol, oracle, or regression
coverage.

- [ ] Extract the secure policy loader, typed representation, validation, and
  normalized inspection API into a narrow shared crate used by `splinterd` and
  `splinterm`; initially retain the daemon API as a compatibility wrapper and
  preserve every ownership, permission, size, shape, and semantic check.
- [ ] Decompose daemon request execution into private access, topology,
  history/search, control, and terminal-action handlers while preserving one
  exhaustive top-level `Request` match and the centralized authorization and
  resource tables.
- [ ] Refactor the Wayland update and draw pipelines into explicit private phases
  for update reduction, frame reconciliation, SHM/backing synchronization,
  overlay composition, and commit finalization while preserving Wayland object
  ownership and the atomic acquire/damage/commit sequence.
- [ ] Split topology-manager command handling into picker,
  creation/materialization, Lair lifecycle, Dojo lifecycle, and tab lifecycle
  handlers while retaining one serialized manager loop and typed outcomes.
- [ ] Move automation-client DTO projection, events/subscriptions, image
  transport/cache, and connection framing/cancellation into private modules,
  re-exporting the existing public API unchanged.
- [ ] Add a canonical exhaustive `AuditOperation::as_str()` to
  `splinterm-protocol` and remove the duplicate automation-client and MCP string
  mappings.
- [ ] Conservatively extract repeated MCP mutation preflight, revision, and
  response plumbing while keeping the closed tool-name match visibly exhaustive
  and authoritative.
- [ ] Document a review rule requiring a cohesion, security, or generated-table
  rationale for new functions over roughly 200–300 lines; use it to prevent new
  orchestration hotspots rather than mechanically splitting existing code.
- [ ] If retained evidence becomes a material clone or navigation problem,
  evaluate Git LFS or versioned external object storage with checked manifests,
  immutable provenance, offline behavior, and tooling migration defined before
  moving any artifact.
