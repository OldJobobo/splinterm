# Third-party work

## foot

Splinterm takes architectural inspiration from foot, the fast Wayland terminal
emulator by Daniel Eklöf and contributors.

- Source: <https://codeberg.org/dnkl/foot>
- License: MIT
- Initial port baseline: foot 1.27.0, commit
  `3c5b584b0eafa772eb4376fb6eaf6643399e190e`
- Architectural decision: [`docs/adr/0001-foot-rust-port.md`](docs/adr/0001-foot-rust-port.md)

Foundational terminal representations in `crates/splinterm-terminal/src/`
are translated from Foot's `terminal.h` and the row initialization behavior in
`grid.c` at the revision above. The affected modules record source-level
provenance in their module documentation.

Foot is Copyright (c) 2019 Daniel Eklöf and is distributed under the MIT
License. Splinterm's MIT `LICENSE` preserves the applicable permission and
warranty terms for these translations.
