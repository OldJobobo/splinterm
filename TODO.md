# TODO

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

- [ ] Define explicit named, pinned, preset-derived, and disposable Lair states.
- [ ] Automatically retire low-value exited `terminal-*` Lairs under a bounded,
  documented policy.
- [ ] Never retire active detached Lairs automatically.
- [ ] Add clear save, pin, restore, and delete actions for Lairs.
- [ ] Clarify how inactive Dojos and their saved launch metadata are restored.
- [ ] Distinguish live, detached, restorable, and expired states in Lair and Dojo
  picker presentation.
- [ ] Preserve privacy: do not persist shell contents, clipboard data, terminal
  input, environment values, or other sensitive terminal state.
- [ ] Add migration, lifecycle, capacity, persistence-failure, and picker tests.
- [ ] Document retention defaults, destructive actions, and recovery behavior.

The existing bounded-history compaction remains the temporary safety mechanism
until this lifecycle design is implemented and reviewed.

## File and image path insertion

- [ ] Support dropping one or more files or images onto a terminal and insert
  their shell-escaped local paths into the focused Splint.
- [ ] Support pasting clipboard image data by saving it with a collision-safe
  filename, then insert the saved image path without changing normal text paste.
- [ ] Define Wayland MIME handling, destination and cleanup behavior, user
  confirmation, bracketed-paste behavior, and clear failure feedback.
- [ ] Test spaces, quotes, Unicode, multiple files, file URIs, unsupported or
  remote sources, clipboard image formats, and cancelled or failed saves.

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

- [ ] Implement [Plan 0032](docs/plans/0032-omarchy-screensaver-integration.md)
  to add Splinterm to the terminals supported by the Omarchy screensaver.
