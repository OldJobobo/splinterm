# ADR 0008: Use a bounded daemon-owned terminal image plane

- **Status:** Accepted — Slice 0 evidence and independent review complete
- **Date:** 2026-07-23
- **Plan:** [Plan 0008](../plans/0008-terminal-image-protocols.md)
- **Evidence:** [Spike 0025](../spikes/0025-terminal-image-contracts.md)
- **Reference:** Foot 1.27.0 commit
  `3c5b584b0eafa772eb4376fb6eaf6643399e190e`

## Context

Sixel and Kitty graphics cross parser recognition, terminal semantics, daemon
lifetime, snapshots, update replay, content transport, CPU composition,
scrollback, panes, resize, and resource accounting. Treating images as renderer
attachments would lose them on detach. Putting pixels in cells or ordinary JSON
updates would multiply storage and wire cost.

The current terminal parser collects OSC and DCS strings into bounded vectors
and ignores APC. That remains suitable for small control strings, but not for
image payloads. The protocol is bounded length-prefixed JSON with an 8 MiB frame
ceiling. The renderer writes premultiplied ARGB8888 into Wayland SHM and already
has byte-aware derived caches.

## Decision

### Canonical semantic model

Add one protocol-independent sparse image plane to `splinterm-terminal`.
Sixel, Kitty, and any later iTerm2 decoder produce the same types:

- `ImageContentId`: daemon-assigned nonzero 64-bit identity;
- `ImageContent`: immutable dimensions, format provenance, alpha mode,
  generation, SHA-256 digest, byte charge, and canonical pixels;
- `ImagePlacement`: application image/placement IDs, content ID, stable row
  anchor, column, source crop, destination cell extent, pixel offsets, signed
  z-index, protocol-specific tie key, creation order where applicable, and
  screen ownership; and
- `ImageStore`: separate normal/alternate catalogs, sparse placement indexes,
  deterministic accounting, and monotonic high-water metrics.

The canonical decoded representation is tightly packed row-major
**premultiplied BGRA8 in sRGB component space**, matching the byte order used by
little-endian Wayland `ARGB8888` backing buffers. Conversion uses
`(component * alpha + 127) / 255`. Opaque RGB receives alpha 255. Dimensions,
stride, area, and byte length use checked arithmetic before allocation.

Pixels are immutable shared backing, not copied into snapshots or updates.
`Cell` remains unchanged and its 24-byte memory-layout assertion remains a
release gate.

### Ownership and lifecycle

The live Splint actor and its terminal own authoritative image semantics and
content lifetime. The graphical client owns only derived source mappings and
scaled surfaces. Derived cache eviction never mutates semantic state.

Normal-screen placements anchor to stable row IDs. Alternate-screen placements
are screen-local and are cleared when entering a fresh alternate screen.
Scrolling, overwrite, erase, clear, history trim, resize, reflow, reset, and
screen switching receive explicit image rules. Sixel follows pinned Foot.
Kitty follows its documented rule that normal text erase does not remove
placements; Kitty delete commands do.

Image state is not persisted across daemon or host restart in Phase 5.
Incarnation replacement invalidates all prior content IDs and transfer tokens.

### Streaming parser and decoder boundary

Graphics payloads never become a complete parser `Vec<u8>`. The VT recognizer
emits bounded begin/data/end/abort events:

- DCS final `q` begins Sixel and streams bytes to its decoder;
- APC is inspected only far enough to recognize `_G`; recognized Kitty control
  bytes and base64 payload are streamed, while unrelated APC remains ignored;
- CAN or SUB aborts immediately to ground; ESC enters normal escape/ST
  handling; only a decoder resource overflow enters discard-through-ST; EOF
  aborts the active decoder at end of stream; and
- current bounded collected OSC/DCS behavior remains for non-graphics strings.

Use pure safe-Rust `png` 0.18 for PNG, `base64` 0.22 for incremental Kitty
payload decoding, and `flate2` 1.1 with its default Rust backend for Kitty
`o=z`. Decoder wrappers enforce Splinterm limits before and during output.
No broad multi-format image crate is accepted.

### Semantic snapshots and updates

Ordinary snapshots and updates carry bounded content metadata, placement
metadata, additions, removals, and image damage. They never carry pixel bodies.
A content reference is `(splint_id, incarnation, generation, content_id,
digest)`. Revision replay remains contiguous; gaps force the same explicit
resnapshot behavior used for text.

Public automation and MCP terminal projections do not expose image bodies by
default. Their frozen schemas are not changed by Phase 5.

### Content transfer

Use a second daemon-owned Unix socket dedicated to image content. The existing
JSON control connection requests missing content and receives a short-lived,
single-use transfer token bound to executable identity, authenticated peer,
Splint, incarnation, content generation, content ID, digest, and byte length.
The client opens the content socket and presents that token. The socket lives
in the owner-only runtime directory with mode `0600`. It repeats ADR 0007's
`SO_PEERCRED` + `SO_PEERPIDFD` + opened-executable-snapshot authentication and
requires an exact match with the control peer.

Tokens are 32 CSPRNG bytes, expire after five seconds, and are atomically
removed before binding validation so mismatch and replay both fail closed.
There are at most four pending tokens per peer, 32 per daemon, eight
unauthenticated content connections, and a 512-byte handshake. Connection and
handshake deadlines are five seconds.

The content channel has two negotiated delivery modes:

1. **Sealed memfd:** create with `MFD_CLOEXEC | MFD_ALLOW_SEALING`, write and
   `fstat` the exact length, apply
   `F_SEAL_WRITE | F_SEAL_GROW | F_SEAL_SHRINK | F_SEAL_SEAL`, verify all seals
   with `F_GET_SEALS`, then send the FD with `SCM_RIGHTS`; receive with
   `MSG_CMSG_CLOEXEC`, map read-only, and verify length/digest.
2. **Binary chunks:** mandatory transport-independent fallback using fixed
   headers and raw chunks no larger than 64 KiB. A receiver advertises a
   four-chunk window, acknowledges contiguous offsets, and may cancel. Digest
   verification completes the transfer.

The content socket does not carry terminal control operations. Malformed,
expired, replayed, stale-incarnation, unknown-content, out-of-window, or
oversized requests close only that transfer. Stalled transfers have bounded
queues and cannot block PTY draining or semantic subscribers.

Descriptor passing stays behind a narrow safe wrapper using `rustix`/`nix`.
If safe APIs cannot express sealing and ancillary-data handling, a dedicated
leaf crate may contain the smallest reviewed unsafe boundary; unsafe code does
not enter terminal, protocol, daemon actor, or renderer modules.

### Accepted practical Kitty subset

The first advertised subset implements direct (`t=d`) chunked transmission for
`f=24`, `f=32`, and `f=100`, optional `o=z`, actions `a=t`, `a=T`, `a=p`,
`a=q`, and `a=d`, image IDs `i`, placement IDs `p`, quiet mode `q`, source crop
`x/y/w/h`, destination `c/r`, cell offsets `X/Y`, cursor policy `C`, and signed
`z` ordering. Query replies are emitted before later PTY input, including DA1,
is processed.

Kitty has three composition tiers: `z < -1073741824` is below non-default cell
backgrounds; other negative z is above cell backgrounds but below text; and
nonnegative z is above text. Equal-z Kitty placements sort by application image
ID, lower first. Creation order is used only when a protocol does not define a
tie key. Trusted Splinterm chrome remains above every tier.

The first delete selectors are visible-all (`d=a/A`) and exact image/optional
placement (`d=i/I`). Unsupported media, selectors, animation, relative or
Unicode placement commands return bounded compatible errors and are not
advertised. File, temporary-file, and POSIX shared-memory application inputs
remain disabled pending their separate security gate.

Replies use printable bounded payloads and preserve Kitty correlation fields.
Success is `OK`; missing content is `ENOENT`; malformed or unsupported fields
use `EINVAL` or `ENOTSUP`; resource limits use `ENOSPC`; oversized payloads use
`E2BIG`; decode failures use `EBADMSG`. Quiet mode suppresses replies exactly as
documented.

### Foot-compatible Sixel subset

Port the pinned DCS `q` path, aspect ratio parameters, opaque/transparent
background mode, raster attributes, repeat introducer, color select/definition
in HLS and RGB, carriage return, graphical newline, trailing-transparent-row
handling, cursor/scroller modes, XTSMGRAPHICS color/geometry queries, overlap,
overwrite, scrollback, resize, and reflow behavior.

The source authority is `sixel.c`, `sixel.h`, `dcs.c`, `terminal.c`, `grid.c`,
and `render.c` at the pinned commit. Exact hashes and fixture seeds are recorded
by Spike 0025.

### Resource budgets

These are semantic limits, not tuning hints:

| Resource | Limit |
|---|---:|
| encoded payload per transmission | 8 MiB |
| decoded pixels per content | 16 MiB / 4,194,304 pixels |
| width or height | 4,096 pixels |
| decoded authoritative content per Splint | 32 MiB |
| decoded authoritative content per daemon | 64 MiB |
| content objects per Splint | 64 |
| placements per Splint | 256 |
| active inbound Kitty uploads per PTY | 1 |
| outbound content transfers per Splint / daemon | 2 / 4 |
| aggregate encoded bytes in flight per daemon | 16 MiB |
| client source cache | 32 MiB |
| client scaled cache | 32 MiB |
| total client image cache | 64 MiB |
| binary chunk / receive window | 64 KiB / 4 chunks |
| Kitty control data / reply text | 1 KiB / 512 bytes |
| encoded chunk / decoded full chunk | 4,096 / 3,072 bytes |
| unchunked direct compatibility payload | 8 MiB |
| pending tokens per peer / daemon | 4 / 32 |
| unauthenticated content connections | 8 |
| token size / TTL | 32 bytes / 5 seconds |
| Sixel colors | 1,024 |
| decoder expansion ratio | 64:1, additionally bounded by decoded limit |
| decoded pixel writes per command | 16,777,216 |

Admission charges allocations before commit. New content is rejected when safe
reclamation of unplaced content cannot satisfy the limit. Existing visible
content is never silently discarded.

The accepted no-image baseline is 49,262,592-byte median and 49,418,240-byte
maximum child-inclusive RSS with zero median idle CPU ticks over ten guarded
samples. Phase 5 no-image closure allows at most 4 MiB or 5% RSS growth,
whichever is smaller, and no increase above the existing one-tick p95 idle
budget. Image-active runs remain within the established 256 MiB graphical RSS
and 128 MiB SHM budgets.

## Rejected alternatives

- Pixel pointers or placement lists in every cell: permanent text-only cost.
- Complete graphics strings in parser vectors: encoded-size and expansion
  amplification before semantic admission.
- Pixel bodies in JSON snapshots/updates: repeated serialization and queue
  amplification.
- JSON base64 content retrieval: retains the amplification the separate channel
  is designed to remove.
- Renderer-owned semantics: breaks detach/reattach and authoritative resync.
- Unbounded cache eviction of visible content: breaks application image IDs.
- GPU rewrite before correctness: expands scope without removing semantic risk.
- Enabling Kitty filesystem transports immediately: turns terminal output into
  daemon filesystem authority before its threat model is accepted.

## Consequences

Phase 5 changes all four major layers but preserves dependency direction:
terminal semantics stay renderer-independent, protocol owns serde projections,
daemon owns admission/transport, and the client owns derived composition.
The second socket and image generations add protocol surface, but keep binary
bodies out of ordinary control frames and preserve a bounded fallback.

The practical milestone is deliberately not full Kitty compatibility. iTerm2,
external Kitty transports, Unicode placeholders, relative placements, and
animation remain later gated work.

Foot permits 10,000×10,000 Sixel geometry and optional linear/10-bit rendering.
Splinterm deliberately advertises a bounded 4,096-per-axis, 4,194,304-pixel,
premultiplied-BGRA8 subset. “Foot-compatible” means accepted behavior within
those advertised bounds, not support for Foot's larger or higher-bit surfaces.
Kitty's document recommends 4,096-byte direct chunks, while `kitten icat 0.48.0`
emits a 5,084-byte unchunked direct PNG payload in the recorded representative
trace. Splinterm accepts one bounded unchunked direct payload up to 8 MiB for
that compatibility; multi-command uploads retain the 4,096-byte encoded chunk
limit, `% 4 == 0` non-final rule, continuation-key restriction, and no
interleaving.

## Validation

Each slice must run its focused tests plus formatting, clippy, workspace tests,
serialized daemon end-to-end tests, parser fuzzing for an explicit duration,
and package/license validation. Graphical validation follows `AGENTS.md`: one
pre-mapped no-focus smoke on inactive workspace 8 / DP-2, then the approved
matrix only after that smoke passes.
