# Runtime memory diagnostic

This artifact records the focused investigation prompted by a real client
reaching 176 MiB RSS after repeated runtime font zoom and an installed daemon
reaching 126 MiB RSS after sustained output.

The client retained all 24 permitted raster-face sizes. Their repeated font-file
mappings accounted for about 50 MiB RSS. Commit `b295e5e` now clears obsolete
persistent glyph and raster-face state whenever a runtime font-size change is
accepted.

The daemon had one dojo, one splint, and one shell, but 13 runtime threads and
six large anonymous mappings consistent with glibc per-thread allocator arenas.
Commit `31f59f8` bounds the asynchronous local daemon to two Tokio workers. An
isolated 12,000-line output smoke then used four total threads, 9.8 MiB RSS, and
no anonymous mapping over 1 MiB. A post-cache-fix client still used 14 threads,
90.2 MiB RSS, and 77.2 MiB PSS; commit `b25f511` now bounds its Tokio runtime to
two workers as well. Further inspection found that the renderer eagerly retained
38.2 MiB of six complete font files. Commit `4bad8e6` loads each face's bytes only
when shaping first uses it, removing 28.8 MiB of CJK and emoji data from ordinary
ASCII startup while preserving exact fallback behavior. A symbolized fresh-client
Heaptrack profile then showed that real shell content activates those fallbacks:
35.1 MiB of its 57.3 MiB peak tracked heap was copied font-file data, while about
14.5 MiB was JSON deserialization. Commit `7a9e382` moves immutable font bytes to
a bounded read-only file-mapping leaf. A non-graphical ASCII/CJK/emoji renderer
profile subsequently peaked at 1.11 MiB tracked heap with no copied font-file
allocation.

The attempted fresh graphical profile escaped workspace 8 and briefly took focus
despite pre-map placement. It was immediately closed, focus returned to Foot, the
pointer did not move, and that launch method was not retried.

See [`summary.json`](summary.json) for machine-readable measurements and explicit
limitations. The installed daemon was intentionally not restarted because that
would terminate its live shell. Real-window post-install client measurement is
therefore still required before treating these numbers as a final replacement
baseline.
