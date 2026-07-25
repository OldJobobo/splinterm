# Plan 0008: bounded terminal image protocols

- **Status:** Complete — Slices 0–6 and 8 accepted; optional Slice 7 deferred
- **Roadmap:** Phase 5 — bounded terminal image protocols
- **Foundation:** [ADR 0001](../adr/0001-foot-rust-port.md),
  [Plan 0001](0001-terminal-kernel.md),
  [Plan 0002](0002-omarchy-terminal-mvp.md), and
  [Plan 0003](0003-phase8.1-closure-execution.md)
- **Reference source:** Foot 1.27.0, commit
  `3c5b584b0eafa772eb4376fb6eaf6643399e190e`
- **Primary target:** practical Kitty graphics compatibility plus Foot-compatible
  Sixel, with bounded memory and no idle cost when images are unused

## Goal

Add a generic semantic image plane to Splinterm, then use it to provide:

1. Foot-compatible Sixel behavior;
2. the Kitty graphics operations needed by common inline-image applications;
3. safe detach, reattach, scrollback, resize, resync, and multi-pane behavior;
4. optional iTerm2 OSC 1337 inline-image compatibility; and
5. a path to full Kitty graphics behavior, including advanced transports and
   animation, without making those features prerequisites for the first useful
   release.

Image support must preserve Splinterm's defining ownership model: `splinterd`
owns canonical terminal semantics while the disposable `splinterm` client owns
Wayland and derived rendering resources. It must not make the daemon depend on
Wayland, enlarge every terminal cell, embed large pixel bodies in ordinary JSON
snapshots, or create an unbounded decoder, cache, queue, scrollback, or animation
store.

## Implementation status

Slices 0 and 1 are implemented and accepted. The production terminal now has a
sparse bounded content/placement plane, stable row anchors, atomic
transmit-and-display, lifecycle integration, image revision damage, bounded
replay/resnapshot behavior, and hard accounting/high-water metrics without
changing `Cell` size.

Slice 2 is implemented and accepted. DCS `q` uses begin/data/end/abort parser
actions rather than the collected DCS buffer. The safe incremental Sixel
decoder covers aspect ratio, transparent/opaque backgrounds, raster attributes,
repeat, palette selection and RGB/HLS definition, carriage return, and
graphical newline under the accepted input, dimension, decoded-byte, and
pixel-write limits. Non-graphical tests match all five pinned-Foot semantic
fixtures for whole input, every split, and bytewise delivery, and prove CAN/SUB
cancellation recovery. Foot's VT340 default palette, configurable Sixel
enablement, private/shared palette behavior, and DEC mode 1070 are implemented,
including shared definitions surviving cancellation and decoder failure.
Foot-compatible cell-aligned overwrite, transparent/partial-cell underlay
composition, stable resize/reflow anchors, and reflow collision resolution are
implemented with bounded fragment fallback. Cursor/scroller placement modes and
bounded XTSMGRAPHICS color/geometry replies are implemented.

The final reviewed guarded matrix under
`docs/spikes/artifacts/0025-terminal-images/slice2-splinterm-sixel-2026-07-25-reviewed/`
matches all five retained Foot final-buffer cells byte-for-byte. Every report
records workspace 8 on DP-2, no initial focus, workspace/window inactivity,
preserved placement, and verified cleanup. A fresh 60-second ASan/libFuzzer
`terminal-advance` run completed 117,896 executions in 61 seconds without a
crash. Focused tests, the serialized 16-case daemon gate, formatting, checksum
validation, and two fresh read-only reviews pass. Workspace-wide Clippy remains
blocked only by unrelated pre-existing benchmark/oracle worktree warnings, and
the broad contract validator reports an unrelated host Kitty-document hash
drift before its retained Sixel checks.

Slice 3 is implemented and accepted. Protocol v23 now carries bounded image capabilities,
limits, metadata, placements, and exact content request/transfer identities.
Attach snapshots and semantic updates project metadata without pixel bodies only
to a trusted UI whose executable identity matches Splinterm; automation remains
image-free by default. The daemon actor resolves immutable pixel backing only
for the exact active-screen content ID, generation, and digest within the
already-bound Splint incarnation; stale and unknown content fail distinctly.
The transfer admission state machine now issues CSPRNG single-use five-second
tokens, consumes them before validation, expires abandoned grants, and enforces
pending-token and active transfer caps with high-water metrics. The mandatory
fallback is connected through a separate mode-0600 Unix socket using raw chunks
of at most 64 KiB, a four-chunk acknowledgement window, exact offsets, digest
verification, cancellation, and five-second I/O deadlines. The trusted client
receiver validates identity and bounds before allocating, retains a bounded
32 MiB exact-identity source cache, and resolves only missing bodies across
attach, update, and resync so repeated placements reuse one transfer. Linux
clients prefer one exactly-sized immutable sealed memfd, require one close-on-exec
received descriptor, recheck all write/grow/shrink/seal seals, map read-only,
and verify the digest; binary chunks remain the negotiated fallback. One shared
64 MiB atomic daemon budget now reserves bytes at catalog insertion and releases
on semantic deletion, reclaim, clear/reset, or actor drop, independently of
pending/active transfer clones. Headless actor tests cover cross-Splint
exhaustion, rejection without mutation, exit release, and replacement admission;
transport tests cover exact sealed-FD passage, raw fallback windows, digest and
cache validation, cancellation-safe active-cap cleanup, and transfer-once cache
reuse. The serialized 16-case daemon suite covers detach/reattach, subscriber
overflow/resync, exit, relaunch/stale incarnation, and socket cleanup. No
graphical command ran for Slice 3.

Slice 4 is implemented and accepted. The trusted client now shares one 64 MiB renderer-wide
exact-identity resident-source registry across panes. Each ordered snapshot or
update carries an atomic exact lease set; leased mapped/buffered entries cannot
be evicted or disappear from byte/high-water accounting. `SnapshotFrame` resolves placements by stable displayed row
ID and retains immutable source handles across cache eviction. The CPU painter
implements crop-before-scale deterministic bilinear sampling with pixel-center
mapping and crop-edge clamping in premultiplied BGRA8 component space,
source-cell-scaled offsets, fractional-scale destination geometry, pane/grid
clipping, premultiplied BGRA
alpha, deterministic application/creation ordering, and the ADR's below-cell,
below-text, and above-text z tiers with cursor and trusted overlays remaining
above image content. Image-bearing semantic changes and scrolls conservatively
force full pane reconstruction; dirty-row painting clears and recomposes all
layers, and image-free scroll-copy remains unchanged. Placement opacity is not a
wire property; Slice 4's accepted opacity behavior is canonical source alpha plus
the existing terminal background opacity.

Slice 4's non-graphical gate covers exact alpha/crop/scale/offset vectors,
byte-exact identity sampling, bilinear upscale/downscale and crop-edge vectors,
fractional 1.25x capture, strict z-tier boundaries and Kitty tie ordering,
stable detached row IDs, pane clipping, cursor/selection precedence, image
removal and all-row full/incremental identity, image-scroll full reconstruction,
active/inactive scale rebuild, and resident-source eviction/admission metrics.
The renderer uses the existing Wayland backing buffer and allocates no scaled
surface cache, so SHM remains unchanged; the enforced 64 MiB resident cap is the
accepted Slice 0 client-cache value recorded in
`docs/spikes/artifacts/0025-terminal-images/budget-probe.json`. The no-image path
retains an empty non-allocating placement vector and the existing image-free
scroll-copy tests. No graphical command ran for Slice 4. A post-acceptance image-quality
correction replaces nearest sampling with the deterministic bilinear contract
above; exact headless vectors cover the filter. An approved guarded live smoke on
DP-2/workspace 8 displayed iTerm2 and Sixel renditions of the same source in
60-by-22-cell boxes without taking focus, and the user visually accepted the
result. Reproducible captured comparison evidence and image-active composition
latency remain Slice 8 evidence.

Slice 5 is implemented and accepted as the practical static-image Kitty subset,
not full Kitty compatibility. Selective APC `_G` recognition streams bounded
control and base64 data without retaining unrelated APC. Direct RGB, RGBA, and
PNG transmission, optional zlib compression, chunk continuation, anonymous
representative-client display, query, placement, visible deletion, crop,
one-sided aspect-preserving destination extents, cell offsets, cursor policy,
and signed z ordering map onto the generic image plane. Encoded uploads share a
16 MiB daemon admission budget and canonical replacements atomically admit only
the byte delta. Unsupported transports, animation, relative placement, Unicode
placeholders, and extra selectors return bounded compatible errors and remain
unadvertised.

The 15 recorded spec-derived cases in `kitty-static-v1.json` execute as tests.
Pinned `kitten icat` 0.48.0 and Chafa 1.18.2 byte streams are retained under
`crates/splinterm-terminal/tests/kitty-data/`, verified against their recorded
SHA-256 digests, and replay without protocol errors. Headless tests cover C1 and
7-bit termination, continuation failures and quiet modes, cross-terminal upload
exhaustion/release, replacement rollback, raw/PNG allocation limits,
visible-versus-scrollback deletion, resize, reflow/scrollback anchors,
alternate screen, revision replay, and reset. No graphical command ran for
Slice 5. After explicit approval to retry the two invocation errors, a corrected
nightly ASan/libFuzzer `terminal-advance` run completed 88,832 executions in 31
seconds with no crash, timeout, or sanitizer finding; final coverage was 2,401
edges and 11,804 features.

Slice 6 is implemented and accepted with a deliberately split result. Bounded
OSC 1337 `File=` inline PNGs stream through selective parser actions into the
generic image plane; ordinary OSC remains collected under its existing small
limit. The accepted subset covers inline-only PNG, exact optional `size`,
discarded validated base64 `name`, cell/pixel/percentage/auto extents, aspect
preservation, default next-line/column-zero cursor movement, the bounded
`doNotMoveCursor` extension, BEL, ESC-ST, and C1-ST. Anonymous content uses
text-overwrite reclamation and the same process-wide encoded and authoritative
budgets as Kitty. Twelve self-contained official-spec-derived fixtures pin the
source document hash, PNG bytes/hash, terminal geometry, terminators, cursor,
and placement results.

The external Kitty input security spike is complete but not accepted for
production. Ambient file, temporary-file, and POSIX-SHM names would turn
attacker-controlled PTY output into daemon filesystem authority without a safe
cross-local/remote/detached namespace or replacement-proof temporary unlink.
`t=f`, `t=t`, and `t=s` therefore remain disabled, unadvertised, and bounded by
`ENOTSUP`; adversarial tests prove they commit no image state and leave named
regular files and symlinks unchanged. A future implementation requires an
authenticated incarnation-bound capability and private cleanup namespace rather
than ambient names. A nightly ASan/libFuzzer `terminal-advance` run completed
39,143 executions in 31 seconds with no crash, timeout, or sanitizer finding;
final coverage was 2,474 edges and 12,224 features. No graphical command ran for
Slice 6.

Slice 8's implementation and runtime gates pass, but final acceptance remains
blocked. The final image-closure candidate guarded matrix under
`docs/spikes/artifacts/0025-terminal-images/slice8-graphical-final/` passes one
Kitty scale smoke, one bounded Sixel scale case, and horizontal and vertical
two-pane Kitty cases. Every report records exact color-region checks, bounded
one-batch decoder/compositor probe timings, daemon/client RSS/PSS/SHM mappings,
content/cache bytes, common release hashes, workspace 8 / DP-2 placement, no
focus, and clean cleanup. The final ten-sample no-image matrix under
`docs/spikes/artifacts/0025-terminal-images/slice8-no-image-idle-2026-07-25-final/`
records 1,773,568 bytes median RSS growth against the 2,463,129-byte allowance
and a one-tick nearest-rank p95/maximum idle CPU result; both gates pass.
Image-token expiry is event-driven only while tokens exist, unchanged themes
avoid JSON parsing in both window paths without changing the 500 ms cadence,
and Thin release LTO keeps RSS inside the accepted envelope.

The complete serialized workspace suite and a fresh 103,027-execution fuzz run
pass; the final matrix records those commands in `VALIDATION.md`. Package-source
manifests and image compatibility, architecture, remote-security, provenance,
and user documentation are updated. Eager default-focus control acquisition now
falls back to an uncontrolled observer when another client owns the exclusive
lease, while retaining eager ownership when available. The clean committed main
and MCP packages build and pass extracted runtime validation, including the
canonical protocol-v23 relay handshake and installed image documentation.
Workspace Clippy's pre-existing Rust 1.91 style findings and installed
Kitty-document hash drift remain separately recorded host/worktree issues and
do not invalidate the accepted focused gates. Slice 8 and Phase 5 are accepted.

## Feasibility and current baseline

The work is feasible, but it is not a renderer-only feature. Images cross the
VT parser, terminal/grid semantics, daemon lifetime, protocol snapshots and
updates, renderer composition, scrollback, reflow, and resource accounting.

The current code has useful boundaries but no image path:

- `crates/splinterm-terminal/src/vt/mod.rs` recognizes OSC and DCS as bounded
  collected strings. APC, which carries Kitty graphics, is consumed as an
  ignored SOS/PM/APC string.
- `crates/splinterm-terminal/src/config.rs` defaults OSC and DCS retention to
  4 KiB. Raising these limits would materialize large encoded bodies and is not
  an acceptable image design.
- `crates/splinterm-terminal/src/terminal.rs` reports every DCS as unsupported.
- terminal snapshots and revisions describe rows, scrolls, cursor, modes,
  palette, dimensions, title, and history, but no image content or placements.
- `crates/splinterm-protocol/src/lib.rs` uses bounded length-prefixed JSON with
  an 8 MiB frame ceiling. Repeating base64 image bodies in snapshots or updates
  would amplify memory and wire size.
- `crates/splinterm/src/renderer.rs` is a CPU compositor with a bounded
  scale-specific glyph cache. `crates/splinterm/src/wayland.rs` retains a CPU
  backing buffer and submits ARGB8888 Wayland SHM buffers.

Pinned Foot already supplies the behavioral baseline for Sixel through
`sixel.c`, `sixel.h`, `dcs.c`, `terminal.c`, `grid.c`, `render.c`, and the
associated CSI/configuration paths. Foot does not supply the Kitty graphics
protocol; Kitty behavior is an intentional Splinterm extension and needs its
own fixtures and compatibility evidence.

## Product decisions fixed by this plan

### One protocol-independent image plane

Sixel, Kitty, and iTerm2 are decoders and command surfaces over one internal
model. They must not create independent renderer stores or unrelated scrolling
rules.

The model separates immutable content from placements:

```text
ImageContent
  content_id
  source format and dimensions
  alpha/color metadata
  bounded immutable backing reference
  encoded and decoded byte charges

ImagePlacement
  placement/application IDs
  content_id
  stable row anchor and column
  source crop and destination extent
  pixel offsets
  z-index and creation order
  scrolling, reflow, and screen ownership
```

Many placements may reference one content object. Content is deduplicated when
this can be done without retaining a second unbounded copy.

### Images do not enlarge every cell

Do not add a pixel pointer, image ID, or placement vector to `Cell`. Image
content and placements are sparse side data associated with the normal or
alternate screen. Existing memory-layout tests must prove the Foot-derived cell
size remains unchanged.

A row-oriented sparse index may accelerate overwrite, erase, damage, and
viewport lookup, but authoritative ownership remains in the image plane.

### The daemon owns semantics; the client owns derived rendering

The live Splint actor owns image IDs, content lifetime, placement semantics,
protocol replies, and authoritative limits. Therefore images remain available
after graphical detach and can be represented on reattach or resync.

The graphical client owns decoded/scaled presentation caches that can be
recreated from daemon content. Client cache eviction must never mutate terminal
semantics. As with current terminal and scrollback bodies, images are not
persisted across daemon or host restart in the first release.

### Graphics strings are streamed

The general parser must not collect a complete image in `Vec<u8>`. Graphics
paths use bounded begin/data/end/abort semantics or an equivalent incremental
decoder boundary while preserving current chunk independence and discard-to-
terminator recovery.

Small OSC strings such as titles and palette operations may retain their current
collected representation. The parser must distinguish APC `G` Kitty graphics
from ignored APC payloads without making all APC strings retainable.

### Metadata and pixel bodies travel separately

Terminal snapshots and updates carry only bounded image metadata, placements,
content references, additions, deletions, and damage. They do not inline a
complete encoded or decoded image body into every snapshot/update.

A transport spike must select a bounded content-transfer mechanism before wire
implementation. The required shape is:

- content IDs in ordinary semantic snapshots and updates;
- explicit bounded demand for missing content;
- chunked, cancellable, backpressured transfer;
- deduplication when several placements or panes reference the same content;
- stale-incarnation and unknown-content rejection; and
- a transport-independent fallback that does not require descriptor passing.

The preferred local optimization is an immutable sealed `memfd` delivered to a
trusted local UI with `SCM_RIGHTS` and mapped read-only. A bounded binary chunk
fallback is required where descriptor passing is unavailable. Existing JSON
base64 may be used only for a deliberately small spike, not as the accepted
large-image transport.

### CPU/SHM rendering lands before any GPU rewrite

The first renderer composites premultiplied-alpha image regions into the
existing CPU backing store. It clips to the pane/grid and damage rectangle and
reuses current Wayland SHM submission.

Composition order must explicitly implement protocol semantics, including:

1. terminal canvas/background;
2. image placements whose z-order belongs behind text;
3. cell backgrounds, glyphs, and decorations;
4. image placements whose z-order belongs above text;
5. cursor and client-local terminal overlays; and
6. trusted Splinterm chrome, consent, authority, search, and pane overlays.

Trusted application chrome always remains distinguishable and cannot be
covered or spoofed by terminal image content.

A GPU renderer, texture atlas, or dmabuf path is a later optimization requiring
independent evidence. It is not required for protocol correctness or the first
release.

### Stable anchors, not circular row indices

Normal-screen placements use stable row identity so scrolling, detached
viewports, history paging, and reattachment do not depend on the grid ring's
current physical index. Alternate-screen placements remain screen-local and do
not enter normal scrollback.

Every operation that moves, overwrites, erases, trims, clears, reflows, resets,
or switches a screen has an explicit image rule. Width reflow must be ported and
differentially checked against Foot for Sixel. Kitty divergence, if needed,
must be documented and covered by tests.

### Memory limits are semantic behavior

Every allocation is charged before it is committed. At minimum configuration
and metrics distinguish:

- encoded bytes in flight;
- decoded bytes per content object;
- decoded bytes per Splint;
- decoded bytes process-wide;
- content and placement counts;
- concurrent transmissions;
- client source-surface cache bytes;
- client scaled-surface cache bytes; and
- animation frame bytes and active frame rate.

Checked arithmetic rejects invalid dimensions before allocation. Decoders have
input, output, expansion-ratio, dimension, and work/deadline limits.

Derived client surfaces may use byte-aware LRU or FIFO eviction. Authoritative
content must not silently vanish while an application-visible image ID remains
valid. Reclaim unplaced content where protocol semantics permit; otherwise
reject new storage with an appropriate terminal reply and leave existing state
coherent.

Provisional spike values are 16 MiB decoded per image, 32 MiB per Splint,
64 MiB of client image cache, four concurrent transmissions, and 256 placements
per Splint. These are hypotheses, not accepted defaults. Slice 0 must replace
them with measured values coordinated with the existing glyph, history, SHM,
and process RSS budgets.

### Kitty compatibility is staged honestly

The first Kitty milestone targets the operations needed by common static-image
clients:

- direct chunked transmission;
- PNG and raw RGB/RGBA payloads;
- transmit, transmit-and-display, and display-existing-content;
- image and placement IDs;
- query and bounded success/error replies;
- deletion by supported selectors;
- source crop, destination size, offsets, alpha, z-index, and creation order;
- cursor/placement behavior;
- scrolling, screen switching, reset, resync, and reattach.

The terminal advertises and acknowledges only implemented behavior. File,
temporary-file, and shared-memory application transports, Unicode placeholders,
animations, frame composition, and less-common selector combinations are later
slices. Until implemented, they return a compatible bounded error rather than
being ignored or falsely reported as supported.

### External image transports require a security decision

Kitty file, temporary-file, and shared-memory transmissions cause the daemon to
open or consume an application-named object. They remain disabled until a
security spike defines namespace behavior, ownership/type checks, size checks,
symlink and replacement handling, temporary-file deletion, cancellation, and
remote/headless behavior.

No terminal image escape may grant automation authority, read arbitrary files
back to an automation client, cover trusted chrome, or turn terminal-derived
paths into privileged instructions.

### Animation is opt-in and strictly bounded

Static images are the first release gate. Animation does not land until frame
storage, timing, hidden/offscreen suspension, reduced-motion behavior, damage,
CPU, and RSS budgets pass independently. At most one timer source per window or
another demonstrably bounded scheduler may drive visible animations; one timer
per retained frame is prohibited.

## Non-goals

- replacing the Foot-derived terminal engine with another terminal crate;
- a GPU renderer as a prerequisite for images;
- persisting image bodies across daemon/host restart;
- embedding image data in every cell, row patch, or semantic update;
- unbounded compatibility with arbitrary dimensions or frame counts;
- decoding image bodies in the Wayland event callback without bounded work;
- allowing terminal content to cover trusted application chrome;
- silent acceptance of unimplemented Kitty operations;
- broadening automation, relay, clipboard, filesystem, or policy authority; or
- claiming full Kitty compatibility at the practical static-image milestone.

## Proposed crate and ownership boundaries

### `splinterm-terminal`

Owns streaming graphics recognition, protocol command semantics, screen-local
image catalogs and placements, stable anchors, erase/overwrite/scroll/reflow
behavior, replies, semantic image damage, snapshots, and update replay.

Decoder-specific modules should remain narrow, for example:

```text
src/image/mod.rs
src/image/store.rs
src/image/placement.rs
src/image/sixel.rs
src/image/kitty.rs
src/image/iterm2.rs
```

Exact layout follows implementation pressure, but generic storage must not
silently depend on Kitty or renderer types.

### `splinterd`

Owns per-Splint and process-wide admission, immutable content backing, decode
work scheduling where needed, content transfer, subscriber resync, and metrics.
Slow or disconnected image consumers cannot block PTY draining.

### `splinterm-protocol`

Carries capability negotiation, bounded image metadata, placement snapshots and
updates, missing-content requests, chunk/fd-transfer metadata, stale-content
errors, and advertised limits. Public automation projections should not include
pixel bodies by default.

### `splinterm`

Owns local missing-content resolution, read-only mappings or bounded buffers,
decoded/scaled caches, pane-clipped composition, image damage, animation timing
when enabled, and cache metrics. Image content is terminal data and remains
below trusted overlays.

### Leaf decoder/transport crates

Unsafe codec, mapping, or descriptor-passing boundaries must remain in small
reviewable leaf crates where safe Rust cannot express the OS/library operation.
Prefer memory-safe Rust decoders with narrow feature sets. Adding a general
image crate with many enabled formats requires dependency and attack-surface
review.

## Dependency-ordered implementation slices

### Slice 0 — contracts, oracle fixtures, and budget spikes

**Work**

- Write an ADR for the semantic image plane, ownership, non-persistence,
  protocol content transfer, and security boundaries.
- Record exact Foot Sixel source functions and produce small deterministic
  semantic/render fixtures from the pinned checkout without modifying it.
- Inventory Kitty operations used by selected representative tools and record
  the exact supported first milestone.
- Spike safe PNG/raw decoding, incremental base64/zlib handling, checked
  dimensions, cancellation, and failure behavior.
- Compare bounded binary chunks with sealed-memfd local transfer while
  preserving a transport-independent fallback.
- Measure renderer and daemon RSS with candidate byte budgets and no images.

**Gate**

No production decoder or wire schema lands until the ADR selects canonical
content representation, transfer shape, ownership, measured budgets, and
failure replies. Idle/no-image RSS must remain within the existing accepted
variance.

### Slice 1 — generic image model and lifecycle

**Work**

- Add content/placement IDs and sparse screen-local storage without changing
  `Cell` size.
- Add byte/count accounting and deterministic admission/reclamation.
- Define stable row anchors and placement ordering.
- Integrate print overwrite, erase, insert/delete characters and lines,
  scrolling, history trim/clear, alternate screen, reset, resize, and reflow.
- Add image damage to revisions, snapshots, and bounded update replay.
- Keep content references shared across placements and snapshots.

**Gate**

Pure non-graphical tests cover every lifecycle operation, arbitrary chunking,
budget exhaustion, revision gaps, and resnapshot. Cell memory-layout baselines
remain unchanged and all stores expose hard high-water metrics.

### Slice 2 — Foot-compatible Sixel

**Work**

- Replace collected DCS graphics payloads with streaming DCS parameters and
  byte delivery.
- Port Foot's Sixel palette, raster attributes, repeat, transparency,
  cursor/scroller modes, geometry queries, and replies.
- Port insertion, overlap/overwrite, scrollback, erase, resize, and reflow
  behavior with provenance.
- Convert completed Sixel data into the generic content/placement model.
- Add configuration compatible with the accepted Foot subset.

**Gate**

Chunk-boundary, malformed-input, query/reply, lifecycle, and semantic
fixtures match pinned Foot. Decoder fuzzing remains bounded, and deterministic
CPU pixel fixtures match the oracle for the accepted matrix.

### Slice 3 — daemon/wire content transport

**Work**

- Negotiate image capabilities and explicit limits.
- Add placement/content metadata to attach snapshots and semantic updates.
- Add bounded missing-content retrieval with cancellation and backpressure.
- Implement the selected local/fallback transfer paths without embedding full
  blobs in repeated JSON events.
- Bind every content request to Splint, incarnation, content ID, and content
  generation.
- Handle detach, reattach, stale clients, slow subscribers, update gaps,
  resync, exit, relaunch, and daemon cleanup.

**Gate**

Headless end-to-end tests prove content is transferred once and referenced
thereafter, stalled consumers do not block PTY reads, old incarnations cannot
retrieve replacement-session content, and every queue/allocation remains
bounded.

### Slice 4 — CPU compositor and damage

**Work**

- Add read-only source surfaces and a renderer-wide byte budget.
- Composite clipped alpha content in exact z/creation order.
- Support source crops, deterministic pixel-center bilinear destination scaling
  with crop-edge clamping over premultiplied BGRA8 components, offsets, panes,
  fractional scale, opacity, and terminal background interaction.
- Integrate image rectangles with full redraw, row damage, scroll-copy, theme
  repaint, font zoom, output scale, selection, cursor, and overlays.
- Ensure inactive panes and detached history display the correct image plane.

**Gate**

Non-graphical final-buffer tests prove alpha, clipping, crop, byte-exact identity
sampling, bilinear upscale/downscale and edge clamping, fractional scale,
z-order, pane isolation, trusted-overlay precedence, and full-versus-incremental
identity.
Image cache eviction cannot alter pixels. CPU/RSS/SHM metrics remain within the
Slice 0 budgets.

### Slice 5 — practical Kitty graphics

**Work**

- Recognize APC `G` without retaining unrelated APC strings.
- Implement direct chunked PNG/RGB/RGBA transmission and compression options
  accepted by the milestone.
- Implement transmit/display/query/delete, IDs, placements, crop, destination
  extent, offsets, z-order, cursor movement, and replies.
- Map Kitty content and placements onto the generic image plane.
- Add capability detection fixtures for representative applications.

**Gate**

Official/spec-derived fixtures and representative static-image clients pass the
advertised subset. Unsupported commands receive bounded compatible errors.
Detach/reattach, resize, scrollback, alternate-screen, pane, and resync tests
preserve image identity and output.

### Slice 6 — iTerm2 compatibility and external Kitty transports

**Work**

- Add streaming OSC 1337 metadata/base64 decoding into the generic plane.
- Complete the external-transport security spike.
- If accepted, implement Kitty file, temporary-file, and shared-memory input
  with exact cleanup and error behavior.
- Keep local application-input transports distinct from daemon-to-UI content
  transport.

**Gate**

Adversarial path/SHM/replacement/oversize/cancellation tests pass. No image
transport can read content back through an unauthorized automation operation or
escape the configured byte and time limits.

### Slice 7 — advanced Kitty placement and animation

**Work**

- Implement remaining deletion/placement selectors and Unicode placeholders if
  still required for the full target.
- Add bounded animation frames, composition, scheduling, visibility suspension,
  reduced-motion behavior, and frame damage.
- Publish an exact compatibility matrix distinguishing implemented, intentionally
  unsupported, and divergent behavior.

**Gate**

Animation CPU, frame pacing, hidden-window idle, RSS, and eviction tests pass.
The project may claim full Kitty graphics compatibility only after every
required command is implemented or an explicit documented compatibility rule
covers it.

### Slice 8 — closure and guarded graphical evidence

**Work**

- Run one guarded graphical smoke on inactive workspace 8 on DP-2.
- Only after that succeeds, run the approved Sixel/Kitty scale and pane matrix.
- Record daemon/client RSS, PSS, SHM, content/cache bytes, decode latency,
  composition latency, frame pacing, and idle wakeups.
- Update configuration, architecture, automation-data, remote, packaging,
  provenance, and user documentation.

**Gate**

All non-graphical suites pass; the guarded graphical matrix passes without
workspace/focus violations; every store has recorded bounds; no-image idle cost
remains negligible; package dependencies and license notices are complete.

## Validation contract for every implementation slice

Each slice records:

- parser/model/protocol/renderer unit tests appropriate to the changed layer;
- malformed, oversized, truncated, cancelled, and arbitrary-chunk input tests;
- exact allocation/count/high-water assertions;
- full-snapshot, incremental-update, update-gap, and resync behavior;
- detach/reattach and incarnation replacement behavior where applicable;
- provenance for translated Foot code and fixtures;
- release-mode CPU and memory evidence when pixel work is introduced; and
- confirmation that no graphical command ran unless the slice explicitly
  reached the guarded graphical gate.

Primary non-graphical commands are expected to include:

```sh
cargo fmt --all --check
cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace
cargo test -p splinterm-terminal
cargo test -p splinterd --test end_to_end -- --test-threads=1
cargo fuzz run terminal-advance
```

Fuzz duration and graphical commands are selected explicitly for the slice; they
are not silently run as part of routine editing.

## Slice 3 validation evidence

- `cargo test -p splinterm-terminal --lib --test images --test memory_layout --test snapshot`
  passed 135 tests across the selected targets.
- `cargo test -p splinterm-protocol`, `cargo test -p splinterm-filemap`,
  `cargo test -p splinterm-automation-client`, and daemon library/binary tests passed.
- `cargo test -p splinterd --test end_to_end -- --test-threads=1` passed 16/16.
- `cargo check --workspace --all-targets`, focused warning-denied Clippy for the
  changed libraries, formatting, and `git diff --check` passed. Workspace-wide
  warning-denied Clippy remains blocked only by unrelated pre-existing/untracked
  benchmark-oracle lint findings outside Slice 3.
- No graphical command ran.

## Principal risks and mitigations

### Decoder resource exhaustion

Compressed/base64 input can amplify into large pixel allocations or excessive
CPU. Reject dimensions and encoded bounds before allocation where possible,
stream decoding, use checked arithmetic, charge output as it grows, cap work,
and abort coherently through the string terminator.

### Semantic loss under memory pressure

Silently evicting an application-visible image breaks Kitty identity and
reattach. Separate authoritative content admission from derived cache eviction;
return explicit storage errors when no protocol-safe reclamation exists.

### Reflow and scrollback drift

Images span pixels and cells while normal history reflows logical lines. Port
Foot's Sixel behavior first, anchor to stable row identity, and require
full-versus-incremental and pre/post-reattach equivalence tests.

### Wire amplification and duplicate ownership

Naive JSON/base64 creates encoded, decoded, serialized, socket, client, scaled,
backing, and SHM copies. Keep blobs out of semantic events, transfer on demand,
share immutable local backing where accepted, and publish per-layer byte
metrics.

### Renderer regressions

Large scaling operations can dominate CPU and damage. Clip to visible damage,
cache only under a byte budget, suspend offscreen animation, and retain the CPU
path until evidence justifies another renderer.

### Filesystem and shared-memory authority

Kitty external transports can turn terminal bytes into daemon file operations.
Keep them disabled until their threat model and exact namespace/cleanup rules
are approved; never expose their bodies through ordinary automation reads.

### False compatibility claims

A static subset is not full Kitty graphics. Negotiate and document exact
capabilities, return errors for unsupported commands, maintain a checked
compatibility matrix, and reserve the full claim for Slice 7 closure.

## Definition of done

The practical image milestone is complete when:

1. Foot-compatible Sixel passes pinned differential fixtures;
2. the documented practical Kitty static-image subset works across scrolling,
   resize, panes, detach/reattach, and resync;
3. terminal snapshots and updates reference bounded content without repeating
   pixel bodies;
4. `Cell` and text-only workload memory baselines do not regress materially;
5. daemon and client image stores, transfers, decoders, and caches have tested
   byte/count/time limits and observable metrics;
6. the CPU compositor produces deterministic full/incremental pixels with
   trusted chrome always above terminal content;
7. malformed or unsupported image commands recover parser synchronization and
   return bounded compatible errors where required;
8. no-image idle CPU and RSS remain within the pre-plan accepted variance;
9. package dependencies, provenance, licenses, configuration, and compatibility
   documentation are complete; and
10. guarded graphical evidence passes under the repository's workspace-8/DP-2
    isolation rules.

Full Kitty completion additionally requires the accepted external transports,
advanced placement operations, Unicode placeholders if required by the target,
and bounded animation/frame composition to pass Slice 7.

## Stop gates

Stop for an explicit architecture or product decision if:

- safe streaming requires making the terminal core depend on Wayland or the
  renderer;
- the chosen wire transport cannot support both backpressure and resync without
  embedding repeated image bodies in JSON;
- accepted static-image budgets materially exceed existing renderer/daemon RSS
  envelopes without user approval;
- a decoder requires broad unsafe or format support outside a narrow leaf;
- Foot Sixel reflow cannot be preserved without changing established text/grid
  semantics;
- Kitty external transports require filesystem authority broader than an
  ordinary same-user terminal process should exercise;
- full compatibility requires trusted chrome to be coverable by terminal
  content; or
- graphical validation cannot obey workspace 8 on DP-2 with pre-map no-focus
  isolation.
