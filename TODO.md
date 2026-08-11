# TODO

## Dojo picker vocabulary

- [x] Make `splinterm dojos` the canonical picker command.
- [x] Rename user-facing **Session Picker** and **Recent Sessions** surfaces to
  **Dojo Picker** and **Recent Dojos**.
- [x] Add canonical `splinterm-dojos` and `splinterm-dojo-picker` executables.
- [x] Retain `splinterm sessions`, `splinterm-sessions`, and
  `splinterm-session-picker` as compatibility aliases.
- [x] Update current desktop metadata, documentation, packaging, and validation.
- [ ] Commit, package, install, and perform guarded graphical acceptance of the
  vocabulary release.

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
