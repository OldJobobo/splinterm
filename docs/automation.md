# Supported automation contracts

> **Phase 4 status:** the local JSON/NDJSON CLI, authorization, persistent policy
> loading, daemon-lifetime audit inspection, dedicated SSH stdio relay, and
> public-CLI reference integration are implemented. Human rendering and raw protocol DTOs remain private interfaces
> and are not compatibility promises.

Splinterm automation uses the owner-only local daemon socket. Remote automation
uses the same daemon operations through `splinterm relay --stdio` over
authenticated SSH; `splinterd` does not expose a network listener. See
[remote.md](remote.md) for the exact executable policy and transport lifecycle.
Transport access, an SSH
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
structs remain private. Public DTOs are explicit conversions from those
internal types. The schemas are handwritten so review can reject accidental
exposure of internal fields; implementation tests must prove emitted DTOs match
the checked-in artifacts.

Every machine record names its schema and major version. Fields may be added
within a major version. Removing a field, changing its type, or reinterpreting
its meaning requires a new major version. A client must reject an unknown major
version rather than guessing.

## Output modes

Human-readable output remains the default. Machine modes reserve stdout:

- `--output json` emits exactly one `splinterm.cli.v1` document and is the only
  machine mode accepted for one-shot commands;
- `--output ndjson` is reserved for subscriptions, where it emits one bounded
  `splinterm.cli.event.v1` record per line; and
- diagnostics and logs go to stderr and never include terminal or input bodies.

One-shot NDJSON and JSON subscriptions are rejected rather than silently changing
record shape. Machine output, schema, and timeout flags are also rejected for the
graphical `window` and `launch` commands and for local policy or relay
administration. Those commands remain human-only; automation uses the
non-Wayland lifecycle commands instead.

A daemon `window` is a logical topology resource. `new-window`,
`close-window`, `rename-window`, and `window-focus-hint` do not map, focus,
move, resize, or close a compositor-native Wayland window. Graphical
`splinterm window` clients are separate disposable processes, and the default
focus field is only a persisted presentation hint. Compositor orchestration is
outside the v1 automation contract.

Request and subscription IDs are client-owned nonzero decimal strings, independent
of private daemon protocol IDs. This avoids precision loss in JSON consumers and
allows a one-shot invocation to use request ID `"1"` even when schema selection,
configuration, socket connection, or handshake fails after successful argument
parsing. Syntactically invalid command lines produce no stdout record, write usage
and diagnostics to stderr, and exit with status 2. An unsupported requested major
may be reported using the highest schema major the executable can safely emit.

UUID resource IDs remain strings. Incarnations and history generations are
positive JSON integers; topology and terminal revisions are nonnegative because
their initial state is revision 0. Every terminal read carries exact Splint, incarnation, and revision
provenance plus explicit truncation state. Terminal snapshots contain semantic
cells, not raw bytes: `content_encoding` is `unicode_scalars`, each cell carries a
Unicode `text` string and a display `width`, and ambiguous byte insertion is not
permitted.

Successful one-shot envelopes contain `data` and cannot contain `error`. Failed
envelopes contain a stable symbolic error, bounded message, and retryability,
cannot contain `data`, and always set `truncated: false` because errors have no
continuation mechanism. The optional `operation` discriminator preserves the
operation-less draft v1 fixtures byte-for-byte. Production output always includes
it; whenever it is present, the schema applies a closed operation-specific
`data`, `resource`, and error payload. Both successful and failed `ping` records,
and `audit_inspect` records, omit `resource`; failed `ping` records also omit
resource-revision hints because the operation exposes no resource data.

### One-shot command and operation inventory

The implemented v1 one-shot names are frozen as follows.

| Command | `operation` |
| --- | --- |
| `ping` | `ping` |
| `list` | `list_dojos` |
| `topology` | `inspect_topology` |
| `inspect SPLINT_ID` | `inspect_splint` |
| `snapshot SPLINT_ID` | `terminal_snapshot` |
| `scrollback SPLINT_ID` | `scrollback_page` |
| `search SPLINT_ID QUERY` | `search_scrollback` |
| `authorization status SPLINT_ID` | `authorization_status` |
| `authorization revoke GRANT_ID --yes` | `revoke_access` |
| `audit` | `audit_inspect` |
| `new NAME [-- ARGV...]` | `create_dojo` |
| `split TARGET_SPLINT_ID ... [-- ARGV...]` | `split_splint` |
| `close SPLINT_ID --yes` | `close_splint` |
| `ratio TARGET_SPLINT_ID RATIO` | `set_split_ratio` |
| `new-window DOJO_ID ... [-- ARGV...]` | `new_window` |
| `close-window WINDOW_ID --yes` | `close_window` |
| `rename-dojo DOJO_ID NAME` | `rename_dojo` |
| `rename-window WINDOW_ID TITLE` | `rename_window` |
| `rename-splint SPLINT_ID TITLE` | `rename_splint` |
| `window-focus-hint WINDOW_ID SPLINT_ID` | `set_window_default_focus` |
| `relaunch SPLINT_ID [-- ARGV...]` | `relaunch_splint` |
| `restore SPLINT_ID` | `restore_splint` |
| `restore-window WINDOW_ID` | `restore_window` |
| `restore-dojo DOJO_ID` | `restore_dojo` |
| `send SPLINT_ID TEXT` | `input` |
| `resize SPLINT_ID COLUMNS ROWS` | `resize` |
| `kill SPLINT_ID --yes` | `kill_splint` |

The NDJSON subscription commands are:

- `subscribe terminal SPLINT_ID --output ndjson`;
- `subscribe topology --output ndjson`; and
- `subscribe control SPLINT_ID --output ndjson`.

JSON is rejected for subscriptions, and NDJSON is rejected for one-shot commands.

Terminal, scrollback, and search responses always carry the exact Dojo, window,
Splint, incarnation, terminal revision, and history generation. Bounded pages
use an opaque base64url continuation cursor; `truncated: true` requires a cursor
and `truncated: false` requires `null`. Resync results have an explicit symbolic
reason and no continuation. Topology mutations carry the committed topology
revision and the exact affected stable IDs. Multi-Splint restores return one
closed result per Splint rather than pretending a single generic acknowledgement
covers partial outcomes.

Grant IDs and audit IDs in CLI DTOs are nonzero decimal strings. Audit inspection
returns bounded, body-free metadata and daemon-lifetime retention state. Public
responses never echo direct argv, input text, or search queries and never expose
private frame tags, protocol request IDs, subscriptions, controllers, or transfer
IDs. Creation, process-start, layout-mutation, acknowledgement, resize,
confirmation, and multi-restore results use separate closed families; generic
families are shared only where their semantics are identical.

### Exit status categories

A successful one-shot exits 0. A syntactically invalid command line emits no JSON
and exits 2. After successful parsing, machine failures emit one closed,
operation-tagged error envelope and use these stable categories:

- 3: authorization, consent, or confirmation failure;
- 4: daemon connection, authentication, handshake, schema, or version failure;
- 5: invalid request or argument, missing/stale resource, controller state, or
  resource-limit failure;
- 6: cancellation or timeout; and
- 70: unexpected internal failure.

A subscription assigns public subscription ID `"1"` and emits its current state
as public sequence 1. Each subsequently emitted record increments a client-owned
public counter; private daemon sequence values are checked internally but are not
exposed or offset. Terminal initial state uses `snapshot`; topology and control
initial states use `topology_snapshot` and `control_snapshot`. On a daemon sequence
gap, subscriber stall, or replaced history, the client emits the next sequence as
`resync_required` and terminates cleanly. Production resync records include a
`stream` discriminator (`terminal`, `topology`, or `control`). Their `resource`
object is the single authoritative current-state location: terminal records carry
exact Splint/incarnation/terminal revision and, for replaced history, history
generation; topology records carry only topology revision; control records carry
only exact Splint/incarnation. The `resync` object carries only the symbolic
reason, so duplicated or conflicting revision state is impossible. Operation-less
legacy draft fixtures and stream-less legacy resync fixtures remain accepted for
v1 compatibility but are never emitted by production. The caller must explicitly
resubscribe; the CLI does not hide a gap by silently reattaching.

## Controller and confirmation rules

Controller leases belong to one daemon connection and are not reusable across CLI
processes. Public input and resize commands therefore use an atomic same-connection
workflow: acquire control, perform the action, and release control during cleanup.
The machine CLI does not expose reusable controller IDs, standalone acquire/release
commands, transfer requests or decisions, or forced takeover. Observation and control-status subscriptions never acquire control.

Machine mode never prompts. It requires `--yes` for killing a process, closing a
Splint, closing a window, and revoking authorization. Create, split, restore,
relaunch of an exited Splint, rename, ratio, focus hint, input, and resize remain
explicit authorized commands but do not require a second confirmation flag.

In-Splint clients pass `--expected-incarnation N` to `split`, `snapshot`, `send`,
and terminal/control subscriptions. The CLI compares that precondition against
its fresh authoritative lookup, then carries the selected incarnation or topology
revision into the daemon request. A relaunch before or during the operation fails
as `stale_incarnation` or `stale_topology`; the command never silently retargets
the replacement process.

## Persistent policy v1

No policy file means no persistent third-party grants. Graphical grant-once
consent remains daemon-lifetime-only and absence of its trusted UI fails closed.

CLI commands that need parent IDs or topology CAS compose their operation with
`InspectTopology`; their policy rules also include `topology_metadata_read` with
a Lair resource selector. Terminal/control subscriptions preflight with
`InspectSplint` and require `topology_metadata_read` for that Splint. The daemon
request matrix in ADR 0007 remains authoritative for each individual request.

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
either an exact incarnation or the conspicuous value `current`. The intended v1
contract is that `dojo` and `window` selectors expand to a bounded set and do
not silently authorize descendants created later. The singleton
`{ "kind": "lair" }` selector is required for operations such as creating a
Dojo that have no pre-existing child resource; it does not select future
Splints for later terminal access.

**Known contract mismatch:** the current `ResourceSelector::matches`
implementation lets a Dojo selector dynamically match later descendant windows
and Splints, and a window selector dynamically match later descendant Splints.
That can unintentionally authorize name, layout, terminal, or process operations
on resources absent when the rule was reviewed, contradicting the contract above
and ADR 0007. This behavior is not a supported future-resource grant. It must be
removed or replaced by a separately reviewed, conspicuous, bounded
descendant-authority design before lifecycle MCP or autonomous agent
orchestration is released.

Schema validation is necessary but not sufficient. The daemon must additionally
reject duplicate rule IDs, expired rules, unsafe ownership or mode, symlinks,
hard links, non-canonical paths, unknown resources, invalid scope/selector
combinations, and limits that exceed protocol ceilings. Any load or reload
failure installs a deny-all persistent-policy generation. Use `splinterm policy
validate PATH` and `splinterm policy inspect PATH` for offline administration
through the same secure loader. `splinterm policy reload` asks the canonical
systemd user service to deliver `SIGHUP`; it does not claim the file was
accepted. Publication is atomic, and existing client sessions are disconnected
so subscriptions and connection-owned controller state cannot survive a
narrowed or rejected generation. See [headless.md](headless.md) for installation,
verification, and lifecycle guidance.

## In-Splint automation context

A process running inside a Splint receives no authority merely because of its
location. `splinterd` overrides and injects `SPLINTERM_DOJO_ID`,
`SPLINTERM_WINDOW_ID`, `SPLINTERM_SPLINT_ID`, and
`SPLINTERM_SPLINT_INCARNATION` into each PTY child. These values are discovery
hints so a CLI-using coding agent can identify its initial logical location
without guessing from topology. They are not credentials, policy selectors,
proof of ancestry, or consent, and the daemon never trusts values presented
back by a client. Relaunch injects the new incarnation.

Automation must reconcile every hint against a fresh authorized `topology`
response. Absence, malformed values, stale incarnation, process exit, or denied
topology access must not broaden policy or cause an adapter to select an
arbitrary Splint. See [integrations.md](integrations.md) for the client-author
checklist, reference picker, and safe shell/`jq` workflows.

## Audit v1

Audit records are bounded metadata with the schema `splinterm.audit.v1` and
retention label `daemon_lifetime`. IDs are monotonic and nonzero during one
daemon lifetime. The daemon retains the newest 1,024 records; paginated reads
must report a retention gap rather than hide it.

The audit schema intentionally has no fields for terminal rows, terminal bytes,
scrollback or clipboard bodies, input bytes, search queries, capability tokens,
environment contents, or complete command arguments. Spawn and restore outcomes
may include only an argument count and redacted executable basename. The
one-shot `audit_inspect` projection serializes audit IDs as decimal strings even
though daemon-private counters remain integers.

## Contract fixtures

Golden fixtures live under `tests/automation/fixtures/`. Each wrapper names one
schema and contains the document being tested. The invalid corpus includes the
security-significant cases of contradictory envelopes, incomplete resync,
wildcard scope, path-only executable identity, and an attempted terminal body in
an audit record.

Validate the draft contracts with:

```bash
uv run --with jsonschema python tools/automation/validate-contract-fixtures.py
```

Later implementation slices must retain these fixtures, add command-specific
examples, validate every emitted machine document, and preserve old v1 fixtures
when adding compatible fields.
