# TODO

## Alpha3 command palette and keymaps

- [ ] Complete [Plan 0033](docs/plans/0033-alpha3-command-palette-and-keymap-closure.md).
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
- [ ] Perform separately approved packaged graphical acceptance for the expanded
  palette, resolved help/labels, reload, both built-in profiles, and
  close-other-tabs semantics.

Arbitrary preset/shell command bindings, plugin-defined trusted commands,
numeric Dojo-selection rows, raw send-prefix, and broad palette redesign remain
outside the alpha3 slice.

## Alpha3 scrollback Enter safety

- [ ] Complete [Plan 0035](docs/plans/0035-alpha3-scrollback-enter-safety.md).
- [x] Make Return and keypad Enter on a historical focused Splint use the
  existing Return-to-Live path and send zero PTY bytes.
- [x] Consume the initiating physical key through release so repeat events cannot
  submit input after the viewport becomes live.
- [x] Preserve normal Enter behavior when already live and preserve all trusted
  modal Enter precedence.
- [x] Add focused press/repeat/release, multi-pane/tab, modal-isolation, redraw,
  and PTY-input regressions.
- [ ] Perform separately approved packaged graphical proof that historical Enter
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
- [ ] Install the current packaged alpha and perform guarded graphical acceptance
  of the picker labels, canonical commands, and compatibility aliases.

Private `SessionPicker*` identifiers may remain until a broader internal rename
is justified; they do not define user-facing product vocabulary.

## Lair retention and saved-workspace lifecycle

- [ ] Complete [Plan 0034](docs/plans/0034-alpha3-saved-lair-layouts.md) for
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
- [ ] Perform separately approved packaged graphical save/preview/restore
  acceptance with unequal nested splits and mixed application/shell leaves.
- [ ] Document retention defaults, proportional size restoration, destructive
  actions, explicit execution, limitations, and recovery behavior.

The existing durable topology already stores tree shape, ratios, launch metadata,
and bounded geometry hints. Plan 0034 productizes that foundation; it does not
promise live-process checkpointing or arbitrary foreground-application replay.

## Alpha3 Wayland file-drop path insertion

- [ ] Complete [Plan 0036](docs/plans/0036-alpha3-wayland-file-drop-path-insertion.md).
- [ ] Accept bounded `text/uri-list` drops with Wayland copy semantics and only
  local regular-file URIs.
- [ ] Capture and revalidate the exact pane, Splint incarnation, tab, controller,
  and input generation; never retarget an asynchronous drop.
- [ ] Insert one deterministic, space-separated POSIX-shell-escaped payload with
  no trailing space or submission bytes.
- [ ] Preserve bracketed paste, modal isolation, all-or-nothing multi-file
  behavior, bounded feedback, and body-free diagnostics.
- [ ] Test spaces, apostrophes, Unicode, leading dashes, multiple files, LF/CRLF,
  malformed encodings, remote hosts, stale targets, limits, and cancellation.
- [ ] Perform separately approved packaged graphical acceptance without moving,
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
- [ ] Give selected Dojo tabs a dedicated exact theme-provided background role,
  painted opaquely without mixing or deriving colors, while retaining
  `ui_accent` as the contrasting selected-tab underline.

## Omarchy integration

- [x] Implement Plan 0032's XDG-only app-ID transport, owned profile, packaged
  launcher helper, and explicit collision-safe activation workflow.
- [ ] Rerun [Plan 0032](docs/plans/0032-omarchy-screensaver-integration.md)
  non-graphical validation on the coherent Alpha3 release state and inspect the
  extracted package.
- [ ] Perform separately approved guarded packaged graphical acceptance for the
  Splinterm-owned, opt-in Omarchy screensaver integration.
