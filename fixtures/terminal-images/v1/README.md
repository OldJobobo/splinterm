# Terminal image test fixtures

Executable protocol and pixel-oracle inputs used by terminal and renderer tests.

- `protocol-fixtures/` contains bounded Sixel, Kitty, and iTerm2 command cases.
- `foot-sixel-oracle/` contains the minimum Foot metadata, state, and framebuffer inputs required for exact pixel comparisons.
- `contracts.json`, `budget-probe.json`, and `representative-clients.json` define executable limits and compatibility cases used by `tools/image-spike/validate_contracts.py`.

These are source-owned test inputs, not benchmark results, graphical acceptance records, or planning artifacts.
