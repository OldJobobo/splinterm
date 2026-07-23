# Spike 0025: terminal image contracts, budgets, and oracle inventory

- **Status:** Blocked — final review found remaining executable evidence gaps
- **Date:** 2026-07-23
- **Decision:** [ADR 0008](../adr/0008-bounded-terminal-image-plane.md)
- **Machine record:** [`artifacts/0025-terminal-images/contracts.json`](artifacts/0025-terminal-images/contracts.json)

## Questions

1. Which exact Sixel implementation is authoritative?
2. Which practical Kitty operations can be advertised honestly first?
3. Can PNG, base64, and zlib decoding remain safe Rust and bounded?
4. How can pixel bodies reach disposable clients without repeated JSON/base64?
5. Which limits fit the accepted daemon/client/SHM budgets?

## Reference inventory

The canonical Foot checkout is `/home/oldjobobo/Playground/foot` at exactly
`3c5b584b0eafa772eb4376fb6eaf6643399e190e`. It was inspected without
modification. The source record includes SHA-256 hashes for `sixel.c`,
`sixel.h`, `dcs.c`, `grid.c`, `render.c`, and `terminal.c`.

The Sixel implementation boundary is:

- `dcs.c:446-470`: DCS hook and streaming `put`/`unhook` callbacks;
- `sixel.c:65-187`: aspect ratio, background mode, palette and decoder setup;
- `sixel.c:1424-2138`: checked growth, raster attributes, repeat, color, CR/GNL,
  and streaming byte dispatch;
- `sixel.c:1131-1422`: trailing-row handling, scroll/cursor placement, overlap,
  and insertion;
- `sixel.c:416-1128`: scroll, overwrite, cache invalidation, and reflow;
- `sixel.c:2140-2228`: XTSMGRAPHICS color and geometry replies;
- `grid.c`: row remapping and Sixel ownership through reflow/resize; and
- `render.c:1507-1681`: clipped CPU composition and text interaction.

Foot exposes Sixel as `DCS q ... ST` and documents XTSMGRAPHICS at
`doc/foot-ctlseqs.7.scd:610-613` and Sixel DCS at lines 791-800.

Kitty 0.48.0's locally installed primary document is
`/usr/share/doc/kitty/html/graphics-protocol.html`. The upstream primary URL
`https://sw.kovidgoyal.net/kitty/graphics-protocol/` returned HTTP 200 during
this spike. The local document is the reproducible reference for this host;
its hash is recorded separately from the live page because upstream content can
change.

## Decoder spike

`cargo info` confirms the selected crates are safe-Rust-capable and below the
workspace Rust 1.88 MSRV:

| Crate | Version | Role | License | Native default |
|---|---:|---|---|---|
| `png` | 0.18 | bounded PNG decode | MIT OR Apache-2.0 | none |
| `base64` | 0.22 | incremental APC payload decode | MIT OR Apache-2.0 | none |
| `flate2` | 1.1 | Kitty RFC 1950 zlib | MIT OR Apache-2.0 | Rust backend |

`tools/image-spike` is an executable safe-Rust prototype with exact dependency
pins. Its tests prove custom PNG allocation limits, checked dimensions,
premultiplied BGRA conversion, cancellation, base64 framing, zlib output caps,
and malformed-input rejection. It does not trust decoder allocation defaults:
the wrapper checks dimensions and output size before its own allocation and
configures `png::Limits` for decoder-internal memory. Zlib and base64 output are
charged incrementally. Partial decode never commits content.

A 4,096×4,096 dimension ceiling alone would permit 64 MiB RGBA. The separate
4,194,304-pixel/16 MiB content ceiling is therefore authoritative. Width,
height, stride, crop, and destination arithmetic must use checked operations.
Sixel repeat and raster attributes are also charged by pixel writes, not only
input bytes.

## Transport spike

Four designs were compared:

| Design | Copies/amplification | Backpressure | Resync | Decision |
|---|---|---|---|---|
| Pixels in snapshots/updates | repeated JSON/base64 and queue clones | poor | duplicates bodies | rejected |
| One-shot JSON retrieval | base64 plus serializer/socket/client copies | bounded only by frame | possible | rejected |
| Binary frames on control socket | low copy | possible | complicates existing JSON decoder and request ordering | rejected |
| Dedicated authenticated content socket | raw fallback plus optional FD | independent bounded flow | token/content generation binding | accepted |

The accepted content socket is sibling daemon infrastructure, not a network
listener. `tools/image-spike` tests out-of-order in-window chunks, out-of-window
rejection, cancellation, contiguous ACK progress, final length/digest, 256-bit
CSPRNG tokens, five-second expiry, peer/content/incarnation binding, atomic
single use, and token caps. It also creates a close-on-exec sealable memfd,
verifies size and all four immutable seals, passes it with `SCM_RIGHTS`, receives
it close-on-exec, and proves writes fail. No first-party unsafe code is used.

The fallback sends 64 KiB raw chunks under a four-chunk receive window. The
content socket repeats ADR 0007 peer/executable authentication, uses mode 0600,
and caps pending handshakes, unauthenticated peers, tokens, and transfer queues.
A stalled content connection cannot occupy actor queues or PTY work.

The memfd path is an optimization, not a correctness dependency. If ancillary
FD passing cannot remain behind safe `rustix`/`nix` APIs, it belongs in one
small audited leaf crate. The binary fallback lands and passes first.

## Practical Kitty milestone

Accepted initial commands:

- direct `t=d` payloads only;
- `f=24` RGB, `f=32` RGBA, `f=100` PNG;
- optional `o=z` compression;
- chunks `m=1` then `m=0`, each encoded payload at most 4,096 bytes, each
  non-final payload divisible by four, and at most 3,072 decoded bytes for a
  full chunk; continuation commands allow only `m` and optional `q`, one Kitty
  upload is active per PTY, and other graphics commands abort/reject it;
- one unchunked direct payload up to 8 MiB for recorded `kitten icat 0.48.0`
  compatibility;
- `a=t`, `a=T`, `a=p`, `a=q`, and `a=d`;
- image `i` and placement `p` IDs;
- quiet mode `q=0/1/2`;
- source `x/y/w/h`, destination `c/r`, offsets `X/Y`, cursor policy `C`, and
  signed z-index `z` with Kitty's three tiers and image-ID equal-z tie rule;
- immediate `a=q` replies before later PTY input such as DA1; and
- delete visible-all `d=a/A` and exact image/placement `d=i/I`.

Retransmitting a nonzero image ID replaces its content and removes its existing
placements. Equal image/placement ID replaces the placement. Negative z-index
is behind text; nonnegative z-index is above text but below trusted chrome.
Image erase/reset/screen behavior follows the primary document.

Deferred commands receive bounded errors: application file/temp/shm transports,
image numbers, extra delete selectors, relative placements, Unicode
placeholders, animation, frame composition, and external transport cleanup.
The compatibility document must never call this subset full Kitty support.

Representative non-graphical traces are recorded in
`artifacts/0025-terminal-images/representative-clients.json`: `kitten icat
0.48.0` uses one direct PNG transmit-and-display command with a 5,084-byte
payload; Chafa 1.18.2 uses chunked direct RGBA with `c/r`, `m`, and `q`; and its
Sixel path uses transparent DCS q, raster attributes, and RGB palettes. These
traces select the static milestone but are not visual evidence.

## Sixel fixture seeds

`fixtures/sixel-v1.json` records deterministic, small commands selected from
pinned Foot semantics:

- opaque 1:1 RGB palette output;
- transparent trailing-row trim;
- raster attributes plus repeat;
- graphical carriage return and newline; and
- HLS palette conversion.

Fixtures carry input bytes as lowercase hex and exact canonical BGRA pixels and
semantic expectations. The executable pinned-Foot harness in
`tools/image-spike/capture_foot_sixel.py` applies a narrow state-dump patch only
to the disposable oracle worktree, then records exact decoded state and final
ARGB8888 buffer output. All five cases match byte-for-byte. Raw buffers,
metadata, semantic state, harness/patch hashes, binary hashes, and isolation
reports are retained under `foot-sixel-captures/` and are rehashed and compared
by `validate_contracts.py`.

The guarded run used only inactive workspace 8 on DP-2, pre-map placement, and
no-focus rules. The one-case smoke passed before the remaining four cases ran;
all windows cleaned up without changing the user's workspace or monitor.

`fixtures/kitty-static-v1.json` records the first control/reply and lifecycle
cases from Kitty 0.48.0's primary document. It includes supported and explicit
unsupported commands so capability claims fail closed.

## Budget measurement

The existing guarded five-terminal idle artifact is the pre-image baseline:

- 10 valid Splinterm samples;
- median child-inclusive RSS: 49,262,592 bytes (46.98 MiB);
- maximum RSS: 49,418,240 bytes (47.13 MiB);
- median idle CPU: 0 ticks; and
- p95 idle CPU: 1 tick.

Existing accepted architecture limits remain 256 MiB graphical RSS and 128 MiB
SHM. A 64 MiB client image cache keeps the observed baseline plus a full cache
near 111 MiB before ordinary workload variation. A 64 MiB process-wide daemon
content ceiling prevents unbounded multiplication across Splints; the 32 MiB
per-Splint ceiling prevents one Splint from consuming it all. In-flight encoded
bytes have a separate 16 MiB process cap.

`budget-probe.json` records separate safe-Rust processes that touch every 4 KiB
page of the candidate 64 MiB daemon and client ceilings. Observed RSS deltas are
67,174,400 bytes for each process; PSS deltas are 67,035,136 bytes for daemon
content and 66,950,144 bytes for the client cache. Added to the 49,262,592-byte
child-inclusive baseline, two full candidate ceilings remain below the existing
256 MiB graphical process-tree limit. Image composition reuses existing Wayland
SHM, so the candidate adds no SHM allocation; integrated closure still measures
real daemon/client/PSS/SHM rather than treating this allocation probe as
throughput evidence.

No-image closure allows at most 4 MiB or 5% RSS growth, whichever is smaller,
and no idle CPU budget increase. These limits are intentionally much lower than
Kitty's documented example quota because Splinterm's daemon persists after the
UI detaches and may own many Splints.

## Decision

ADR 0008 remains proposed and Slice 1 remains closed. Final review confirmed
the exact Foot fixtures and token-expiry fixes, but still requires complete
capture-provenance validation, strict per-transfer and socket/resource
admission enforcement, and PNG expansion/cancellation evidence. Binary content
fallback precedes memfd optimization. External application transports and
animation remain separately gated.

## Validation

```sh
python tools/image-spike/validate_contracts.py
cargo fmt --manifest-path tools/image-spike/Cargo.toml --all -- --check
cargo clippy --manifest-path tools/image-spike/Cargo.toml --all-targets -- -D warnings
cargo test --manifest-path tools/image-spike/Cargo.toml
python tools/image-spike/run_budget_probe.py \
  docs/spikes/artifacts/0025-terminal-images/budget-probe.json
(cd /home/oldjobobo/Playground/foot && git diff --exit-code && \
  test "$(git rev-parse HEAD)" = 3c5b584b0eafa772eb4376fb6eaf6643399e190e)
cargo test -p splinterm-terminal --test memory_layout
```

Graphical evidence was generated only through the guarded pinned-Foot harness
on workspace 8 / DP-2 after its one-case smoke passed. Every report requires
both `semantic_exact` and `viewport_origin_matches`; the contract validator
independently rehashes and compares all five retained buffers and state dumps.
