# Supported automation contracts

> **Phase 4 status:** authorization, persistent policy loading, and daemon-lifetime
> audit inspection are implemented behind the private protocol. Public JSON/NDJSON
> commands and the SSH relay are not implemented yet. Until those later slices
> land, the existing human CLI and protocol DTOs remain private interfaces and
> are not compatibility promises.

Splinterm automation uses the owner-only local daemon socket. Remote automation
will use the same operations through `splinterm relay --stdio` over authenticated
SSH; `splinterd` will not expose a network listener. Transport access, an SSH
login, or the same Unix UID does not grant terminal authority.

The security and operation-to-scope contract is fixed by
[ADR 0007](adr/0007-supported-automation-policy.md). Terminal and scrollback
content is always untrusted data. It cannot approve consent, change policy, or
be interpreted as an instruction by a client.

## Public and private boundaries

The public compatibility boundary consists of:

- JSON documents emitted by `--output json`;
- NDJSON records emitted by `--output ndjson`;
- the checked-in Draft 2020-12 schemas under `dist/schemas/v1/`; and
- documented command behavior, exit codes, limits, and cancellation semantics.

Raw daemon frames, protocol versions, Rust enum layouts, and serialized runtime
structs remain private. Public DTOs will be explicit conversions from those
internal types. The schemas are handwritten so review can reject accidental
exposure of internal fields; implementation tests must prove emitted DTOs match
the checked-in artifacts.

Every machine record names its schema and major version. Fields may be added
within a major version. Removing a field, changing its type, or reinterpreting
its meaning requires a new major version. A client must reject an unknown major
version rather than guessing.

## Output modes

Human-readable output remains the default. Machine modes reserve stdout:

- `--output json` emits exactly one `splinterm.cli.v1` document;
- `--output ndjson` emits one bounded `splinterm.cli.event.v1` record per line
  and is required for subscriptions; and
- diagnostics and logs go to stderr and never include terminal or input bodies.

Request and subscription IDs are nonzero decimal strings in public records.
This avoids precision loss in JSON consumers. UUID resource IDs remain strings;
incarnations and revisions are positive JSON integers. Every terminal read
carries exact Splint, incarnation, and revision provenance plus explicit
truncation state.

Successful one-shot envelopes contain `data` and cannot contain `error`. Failed
envelopes contain a stable symbolic error, bounded message, and retryability and
cannot contain `data`. A resynchronization event states why incremental delivery
stopped and supplies the current known revision or history generation.

## Persistent policy v1

No policy file means no persistent third-party grants. Graphical grant-once
consent remains daemon-lifetime-only and absence of its trusted UI fails closed.

A v1 rule has:

- a unique bounded rule ID;
- an absolute canonical executable path and the SHA-256 digest of the exact
  executable file;
- one or more closed operation scopes from ADR 0007;
- explicit resource selectors;
- operation limits; and
- an optional Unix expiry time.

There are no wildcard scopes, basename identities, path-only identities, or
implicit future resources. A `splint` selector identifies an exact Splint and
either an exact incarnation or the conspicuous value `current`. `dojo` and
`window` selectors expand once to a bounded set during authorization. The
singleton `{ "kind": "lair" }` selector is required for operations such as
creating a Dojo that have no pre-existing child resource; it does not select
future Splints for later terminal access.

Schema validation is necessary but not sufficient. The daemon must additionally
reject duplicate rule IDs, expired rules, unsafe ownership or mode, symlinks,
hard links, non-canonical paths, unknown resources, invalid scope/selector
combinations, and limits that exceed protocol ceilings. Any load or reload
failure installs a deny-all persistent-policy generation. `splinterd` reloads
the configured policy on `SIGHUP`; publication is atomic, and existing client
sessions are disconnected so subscriptions and connection-owned controller
state cannot survive a narrowed or rejected generation.

## Audit v1

Audit records are bounded metadata with the schema `splinterm.audit.v1` and
retention label `daemon_lifetime`. IDs are monotonic and nonzero during one
daemon lifetime. The daemon retains the newest 1,024 records; paginated reads
must report a retention gap rather than hide it.

The audit schema intentionally has no fields for terminal rows, terminal bytes,
scrollback or clipboard bodies, input bytes, search queries, capability tokens,
environment contents, or complete command arguments. Spawn and restore outcomes
may include only an argument count and redacted executable basename.

## Contract fixtures

Golden fixtures live under `tests/automation/fixtures/`. Each wrapper names one
schema and contains the document being tested. The invalid corpus includes the
security-significant cases of contradictory envelopes, incomplete resync,
wildcard scope, path-only executable identity, and an attempted terminal body in
an audit record.

Validate the draft contracts with:

```bash
python -m pip install jsonschema
python tools/automation/validate-contract-fixtures.py
```

Later implementation slices must retain these fixtures, add command-specific
examples, validate every emitted machine document, and preserve old v1 fixtures
when adding compatible fields.
