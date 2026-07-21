# Third-party work

## foot

Splinterm takes architectural inspiration from foot, the fast Wayland terminal
emulator by Daniel Eklöf and contributors.

- Source: <https://codeberg.org/dnkl/foot>
- License: MIT
- Initial port baseline: foot 1.27.0, commit
  `3c5b584b0eafa772eb4376fb6eaf6643399e190e`
- Architectural decision: [`docs/adr/0001-foot-rust-port.md`](docs/adr/0001-foot-rust-port.md)

Terminal representations, grid behavior, parser recognition, and command
semantics in `crates/splinterm-terminal/src/` are translated from Foot's
`terminal.h`, `grid.h`, `grid.c`, `vt.h`, `vt.c`, `terminal.c`, `commands.c`,
`csi.c`, `osc.c`, `dcs.c`, `slave.c`, `render.c`, and `reaper.c` at the
revision above. The affected modules
record source-level provenance in their module documentation.

`crates/splinterm/src/box_drawing.rs` is a narrow safe-Rust translation of
Foot's `box-drawing.c` at the same revision. It currently covers U+2500,
U+250C, U+2510, U+253C, and the U+2800–U+28FF Braille range used by the
renderer evidence and full-screen TUI path.

Foot is Copyright (c) 2019 Daniel Eklöf and is distributed under the MIT
License. Splinterm's MIT `LICENSE` preserves the applicable permission and
warranty terms for these translations.

## rmcp

Plan 0007 Slice 0 uses the official Rust Model Context Protocol SDK in the
non-shipping `splinterm-mcp` spike.

- Source: <https://github.com/modelcontextprotocol/rust-sdk>
- Version: exactly 2.2.0 (`rmcp` and `rmcp-macros`)
- License: Apache-2.0
- Default features: disabled
- Enabled rmcp features: `macros`, `schemars`, `server`, `transport-io`
- Protocol accepted by the spike: exactly MCP `2025-11-25`

The selected feature tree contains no HTTP, SSE, OAuth, JWT, client,
child-process, tower, or elicitation dependency. The exact resolved dependency
and license inventory, feature-tree commands, and the reason the workspace MSRV
rose to Rust 1.88 are recorded in
[`docs/spikes/0021-mcp-sdk.md`](docs/spikes/0021-mcp-sdk.md). The crate has
`publish = false` and is not included in packaging.

## rustix

Splinterm uses `rustix` for safe Linux PTY, termios, process, signal, and
owned-file-descriptor operations in `splinterm-pty`.

- Source: <https://github.com/bytecodealliance/rustix>
- Version: 1.1.4
- License: Apache-2.0 WITH LLVM-exception OR Apache-2.0 OR MIT

## fontdb

Splinterm's Roadmap Phase 2 font discovery spike uses `fontdb` to inventory and
query installed font faces.

- Source: <https://github.com/RazrFalcon/fontdb>
- Version: 0.23.0
- License: MIT

## swash

The font-stack spike uses Swash to inspect OpenType metrics and character-map
coverage. Shaping and raster suitability remain under evaluation.

- Source: <https://github.com/dfrg/swash>
- Version: 0.2.10
- License: Apache-2.0 OR MIT

## FreeType and freetype-rs

Phase 8.1 uses the system FreeType library through the safe `freetype-rs`
wrapper in the dedicated `splinterm-freetype` crate. The bridge reproduces the
pinned Foot/fcft light-hinted normal-grayscale raster path and returns only
bounded owned glyph data.

- FreeType source: <https://freetype.org/>
- Reference-host FreeType version: 2.14.1 (`pkg-config` 26.6.20)
- FreeType license: FreeType License (FTL) OR GPL-2.0-only
- freetype-rs source: <https://github.com/PistonDevelopers/freetype-rs>
- freetype-rs version: 0.38.0
- freetype-rs license: MIT
- freetype-sys version: 0.23.0
- Linkage: dynamic system FreeType discovered through `pkg-config`

No first-party unsafe block is used. Exact raster fixtures remain sensitive to
the FreeType build, font file, face index, fontconfig policy, and pixel size.

## smithay-client-toolkit

Splinterm's Roadmap Phase 2 native Wayland mechanism spike uses
`smithay-client-toolkit` for registry dispatch, xdg-shell lifecycle, seat/output
state, keyboard integration, SHM slot pooling, and calloop integration.

- Source: <https://github.com/smithay/client-toolkit>
- Version: 0.20.0
- License: MIT

## wayland-client

The native Wayland mechanism spike uses the Rust `wayland-client` bindings for
Wayland connections and protocol objects.

- Source: <https://github.com/Smithay/wayland-rs>
- Version: 0.31.14
- License: MIT

## unicode-width

Splinterm uses `unicode-width` for safe, deterministic Unicode display-width
classification without FFI or unsafe code.

- Source: <https://github.com/unicode-rs/unicode-width>
- Version: 0.2.2
- License: MIT OR Apache-2.0
