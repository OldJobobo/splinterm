# Plan 0016 multiplexer publication review

Status: **approved for publication**
Seed: `13372075` · warmups: 3 · measured samples per stack/topology: 10

Values below are medians. Native and nested values measure complete stacks; Foot overhead is not subtracted. Visible-marker polling is a screenshot approximation, not compositor presentation latency. Results describe this host and build only.

## Startup, idle footprint, and ANSI output

| Stack | Topology | Children ready | Windows mapped | Idle total RSS | ANSI write complete | ANSI visible marker |
|---|---|---:|---:|---:|---:|---:|
| splinterm-native | single | 86.4 ms | 264.2 ms | 53.1 MiB | 43.6 ms | 617.4 ms |
| splinterm-native | two-columns | 96.0 ms | 302.8 ms | 72.7 MiB | 40.7 ms | 607.2 ms |
| splinterm-native | four-grid | 118.5 ms | 350.2 ms | 111.0 MiB | 78.8 ms | 628.5 ms |
| foot-bare | single | 94.6 ms | 100.1 ms | 50.9 MiB | 4.5 ms | 292.1 ms |
| foot-bare | two-columns | 154.8 ms | 178.1 ms | 99.7 MiB | 5.2 ms | 313.2 ms |
| foot-bare | four-grid | 283.5 ms | 344.0 ms | 197.0 MiB | 6.2 ms | 380.2 ms |
| foot-tmux | single | 77.2 ms | 175.9 ms | 66.1 MiB | 9.9 ms | 308.8 ms |
| foot-tmux | two-columns | 77.6 ms | 176.4 ms | 84.8 MiB | 15.6 ms | 298.6 ms |
| foot-tmux | four-grid | 86.8 ms | 196.8 ms | 122.1 MiB | 25.8 ms | 299.3 ms |
| foot-zellij | single | 153.0 ms | 260.4 ms | 138.2 MiB | 5.4 ms | 287.4 ms |
| foot-zellij | two-columns | 166.0 ms | 333.0 ms | 159.9 MiB | 9.6 ms | 590.2 ms |
| foot-zellij | four-grid | 198.0 ms | 473.1 ms | 203.5 MiB | 29.6 ms | 588.6 ms |

## Interaction and lifecycle

| Stack | Topology | Input to child | Input visible marker | 12-step outer resize | Divider resize | Detach/reattach | Child exit settled |
|---|---|---:|---:|---:|---:|---:|---:|
| splinterm-native | single | 15.6 ms | 814.3 ms | 4462.6 ms | N/A | 519.1 ms | 33.9 ms |
| splinterm-native | two-columns | 15.3 ms | 815.4 ms | 4690.2 ms | 508.9 ms | 594.2 ms | 34.0 ms |
| splinterm-native | four-grid | 15.6 ms | 807.1 ms | 4953.2 ms | 486.4 ms | 648.1 ms | 34.5 ms |
| foot-bare | single | 14.3 ms | 422.6 ms | 4337.8 ms | N/A | N/A | 26.4 ms |
| foot-bare | two-columns | 13.8 ms | 443.8 ms | 4910.3 ms | N/A | N/A | 31.4 ms |
| foot-bare | four-grid | 14.4 ms | 502.0 ms | 5568.0 ms | N/A | N/A | 41.2 ms |
| foot-tmux | single | 14.5 ms | 417.4 ms | 4464.3 ms | N/A | 444.8 ms | 28.8 ms |
| foot-tmux | two-columns | 13.9 ms | 408.0 ms | 4489.1 ms | 361.3 ms | 445.2 ms | 49.7 ms |
| foot-tmux | four-grid | 13.9 ms | 401.6 ms | 4496.8 ms | 355.2 ms | 477.1 ms | 29.0 ms |
| foot-zellij | single | 14.7 ms | 422.7 ms | 5378.8 ms | N/A | 539.3 ms | 56.2 ms |
| foot-zellij | two-columns | 14.4 ms | 497.0 ms | 6509.9 ms | 514.2 ms | 636.0 ms | 79.1 ms |
| foot-zellij | four-grid | 14.7 ms | 415.8 ms | 8153.1 ms | 648.0 ms | 819.1 ms | 170.0 ms |

## Current bare-terminal idle control

Splinterm uses a prestarted daemon; Foot, Kitty, Ghostty, and Alacritty use standalone process launches. Startup boundaries are therefore observed independently rather than treated as identical launch models.

| Terminal | Child ready | Window mapped | Idle RSS |
|---|---:|---:|---:|
| splinterm | 76.7 ms | 135.6 ms | 52.9 MiB |
| foot | 75.6 ms | 56.9 ms | 50.9 MiB |
| kitty | 250.0 ms | 135.1 ms | 345.1 MiB |
| ghostty | 328.5 ms | 241.5 ms | 298.6 MiB |
| alacritty | 150.6 ms | 115.2 ms | 224.5 MiB |

## Evidence checks

- Multiplexer matrix: 36 warmup and 120 measured reports; all valid with exact cleanup.
- Five-terminal idle control: 15 warmup and 50 measured cases; all valid with guarded cleanup.
- Multiplexer source bundle: 183 checksum entries verified before this review bundle was generated.
- Corrected post-processing source, test, input hashes, and generation command are retained in this bundle.
- Unsupported independent-Foot divider and detach semantics remain explicit N/A results.
- The earlier guarded focus stop is not treated as a performance sample; the successful immutable plan reused 40 valid cells and completed the remaining schedule under the same execution identity.
