# Terminal semantic fixture schema v1

Fixtures are JSON documents consumed by both the future Foot oracle and the
Rust terminal parity tests.

## Required fields

- `schema`: integer `1`.
- `id`: stable lowercase kebab-case identifier matching the filename.
- `description`: concise behavior under test.
- `reference`: pinned Foot version, commit, and verification status.
- `initial`: terminal dimensions and relevant initial configuration.
- `input_hex`: exact bytes as lowercase hexadecimal without separators.
- `expected`: normalized semantic terminal state after all input is consumed.
- `intentional_divergences`: empty for parity fixtures, otherwise documented.

## Verification status

- `source_reviewed`: expectation derived from inspection of pinned Foot source.
- `oracle_verified`: produced and confirmed by the automated Foot state adapter.
- `intentional_divergence`: differs from Foot under an accepted Splinterm ADR.

Only the oracle workflow may promote a fixture from `source_reviewed` to
`oracle_verified`.

## Coordinates

Rows and columns are zero-based. Cursor state includes `last_column_flag`,
Foot's deferred-wrap state.

## Rows

`expected.rows` contains every visible row. `text` has exactly `columns`
Unicode scalar values for the current ASCII fixtures. Later fixtures with wide,
combining, spacer, or non-printing cells will add explicit `cells` entries.

`linebreak` follows Foot's row metadata:

- `true`: hard/default row boundary;
- `false`: soft-wrapped continuation.

## Attributes

Default attributes are implicit. `attribute_runs` records only non-default
ranges using half-open columns `[start, end)`.

Supported v1 attribute keys are:

- `bold`, `dim`, `italic`, `underline`, `blink`, `reverse`, `conceal`,
  `strikethrough`;
- `foreground` or `background` with `source` and `value`.

Color sources use `default`, `base16`, `base256`, or `rgb`.

## Events

`expected.events` records semantic side effects in order. Initial fixtures use
an empty array. Later event objects will cover PTY replies, title changes, bell,
and other externally visible effects.

## Chunking invariant

Every fixture must produce the same expected state when input is fed:

- as one complete buffer;
- one byte at a time;
- at every possible single split point;
- using deterministic pseudo-random chunks.
