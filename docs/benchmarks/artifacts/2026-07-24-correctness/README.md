# Splinterbench correctness report

Overall non-graphical validation: **PASS**  
Repository revision: `4753060789a7d06289efa4125eb76536e25f169c` (dirty worktree)  
Behavioral oracle: Foot 1.27.0 `3c5b584b0eafa772eb4376fb6eaf6643399e190e`

Correctness is reported separately from performance. Portable observations for other terminals do not expose or prove private terminal state.

## Oracle parity

- Semantic fixtures: **5/5 covered** by the Rust fixture consumer, including chunking invariance.
- base-final-buffer: **16/16 exact** (`docs/spikes/artifacts/0017/slice1-final-buffer/summary.json`)
- decoration-cursor: **6/6 exact** (`docs/spikes/artifacts/0017/slice3-decoration-cursor/summary.json`)
- font-matrix: **96/96 exact** (`docs/spikes/artifacts/0017/slice4-font-matrix-final/summary.json`)
- scale-fallback-integration: **3/3 exact** (`docs/spikes/artifacts/0017/slice4-graphical-final/summary.json`)

## Non-graphical checks

| Check | Status |
|---|---|
| semantic-vector-sync | passed |
| semantic-fixture-validation | passed |
| oracle-comparator-tests | passed |
| terminal-correctness-tests | passed |
| workspace-oracle-provenance (informational) | failed |

## Feature coverage

| Feature | Status |
|---|---|
| unicode-width-combining-emoji | covered |
| sgr | covered |
| alternate-screen | covered |
| cursor-and-erase | covered |
| resize-and-reflow | covered |
| title-and-pty-replies | covered |
| malformed-sequence-recovery | covered |
| parser-fuzzing | available-not-run |
| hyperlinks-osc-8 | unsupported |

## Graphics capability matrix

Statuses are evidence-bounded: `unknown` is not treated as unsupported or as a zero-performance result.

| Capability | Splinterm | Foot | Kitty | Ghostty | Alacritty |
|---|---|---|---|---|---|
| sixel | partial | unknown | unknown | unknown | unknown |
| kitty-graphics | unsupported | unknown | unknown | unknown | unknown |
| iterm2-images | unsupported | unknown | unknown | unknown | unknown |

## Portable external observations

| Observation | Terminals | Cases | Claim boundary |
|---|---:|---:|---|
| output-marker | 5 | 150 | A high-contrast completion marker became externally visible; screenshot polling does not prove intervening cell content. |
| settled-resize | 5 | 50 | The compositor reported every requested settled geometry; private grid/reflow state was not inspected. |
| child-exit | 5 | 50 | Child exit and window/process lifecycle were externally observed; retained private terminal state was not inspected. |

## Explicit limits

- The checked-in parser fuzz target was not executed by this report and is not claimed as a fuzz pass.
- Hyperlink handling and unsupported graphics protocols are not scored as failed performance runs.
- Graphical Foot reference captures were not regenerated.
