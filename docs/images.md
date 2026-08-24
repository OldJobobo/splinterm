# Terminal image compatibility

Splinterm implements a bounded static-image plane owned by `splinterd` and
rendered by the disposable `splinterm` Wayland client. Pixel bodies do not ride
inside ordinary terminal snapshots, public automation JSON, audit records, or
SSH relay metadata.

## Compatibility matrix

| Protocol/operation | Status | Notes |
| --- | --- | --- |
| Sixel DCS `q` | supported | Foot 1.27.0-compatible streaming decode, palette modes, raster attributes, repeat, carriage return, graphical newline, cursor/scroller modes, overlap, resize, and reflow |
| Kitty direct RGB/RGBA | supported | direct and chunked static transmission; optional zlib |
| Kitty direct PNG | supported | bounded PNG decode; `kitten icat` 0.48.0 and Chafa 1.18.2 streams are pinned fixtures |
| Kitty transmit/display/query | supported subset | IDs, placements, crop, aspect-preserving extents, cell offsets, cursor policy, and signed z tiers |
| Kitty delete | supported subset | visible-all and exact image/placement lifecycle used by the practical static subset |
| iTerm2 OSC 1337 `File=` | supported subset | inline PNG only; cell/pixel/percentage/auto extents and `doNotMoveCursor` |
| Kitty file, temporary-file, and POSIX-SHM media | intentionally unsupported | returns bounded `ENOTSUP`; terminal output never grants daemon filesystem or global SHM authority |
| Kitty Unicode placeholders and relative placement | deferred | optional Slice 7 work |
| Animation/frame composition | deferred | optional Slice 7 work; no full Kitty graphics claim |
| Multipart iTerm2 and non-PNG formats | deferred | not advertised |

“Practical Kitty static-image compatibility” is the release claim. Splinterm
does not claim full Kitty graphics compatibility.

## Bounds and ownership

- one canonical daemon image-content budget: 64 MiB process-wide;
- one terminal may retain at most 32 MiB of canonical image content;
- encoded Kitty/iTerm2 uploads: 16 MiB process-wide;
- trusted renderer resident-source cache: 64 MiB;
- image content transfers use single-use five-second tokens with bounded pending
  and active counts;
- Linux local clients prefer exactly sized sealed memfds; a mode-0600 binary
  socket with bounded chunks, acknowledgement windows, and deadlines is the
  fallback;
- composition reuses the existing CPU backing and Wayland SHM buffers instead
  of allocating a second image surface cache.

Malformed, oversized, truncated, cancelled, or unsupported commands fail
closed and parser synchronization resumes at the next accepted control
boundary. External media names are never opened, unlinked, or mapped.

## Rendering and panes

Images are anchored to stable terminal row identity. Scrollback, resize,
reflow, alternate-screen transitions, detach/reattach, resync, and text
replacement use protocol-independent lifecycle rules. The CPU compositor clips
to each pane and applies crop-before-scale bilinear sampling in premultiplied
BGRA8. Trusted pane chrome, cursor, selection, consent, and search overlays stay
above terminal-controlled image content.

A pane that does not own resize control does not mutate daemon terminal pixel
geometry. Sixel emitted before pixel geometry is available is rejected rather
than guessed. Kitty static placement remains visible in active and inactive
panes because its accepted placement dimensions are explicit terminal cells.

## Verification

Sixel behavior is regression-tested against Foot 1.27.0 commit
`3c5b584b0eafa772eb4376fb6eaf6643399e190e`. Executable protocol and pixel-oracle
fixtures live under `fixtures/terminal-images/v1/`; benchmark runs, graphical
captures, and acceptance records are maintainer material and are not shipped in
the public repository.

The strict no-image RSS and one-tick p95 idle CPU gates pass after release Thin
LTO, event-driven token expiry, and unchanged-theme fingerprinting. Clean
committed main and MCP packages pass extracted runtime validation. Eager
default-focus control acquisition falls back to an uncontrolled observer when
another client already owns the exclusive lease. Phase 5 is complete.
