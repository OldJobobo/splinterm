# Plan 0030: bounded client diagnostic logging

- **Status:** Complete — non-graphical validation and read-only review passed (2026-08-10)
- **Date:** 2026-08-10
- **Revised:** 2026-08-10
- **Scope:** graphical `splinterm` client diagnostics, daemon correlation, crash discovery, retention, and privacy boundaries
- **Related:** [Plan 0017](0017-inline-session-picker-overlay.md), [Plan 0019](0019-dojo-tabs.md), [Plan 0025](0025-command-palette-and-tab-context-menus.md)

## Decision

Add low-volume structured diagnostics to every graphical Splinterm client. The
normal sink is a private bounded per-process file, with warning and error events
also submitted to journald when available. Clean, uneventful logs are removed;
logs containing a warning, error, panic, or graceful termination signal are
retained under a strict count and size budget.

The design follows the useful parts of Alacritty's on-demand per-process log,
WezTerm's runtime log directory and module filters, Foot's warning-level default,
and systemd's journal/coredump integration.

Diagnostic logging is not terminal recording. Splinterm must never log PTY
contents, key text, clipboard contents, command arguments, environment values,
OSC payloads, image bodies, shell working directories, terminal-controlled
names, or third-party error text by default.

## Problem

The daemon is a systemd user service and its `tracing` output reaches journald.
A graphical client launched through the desktop/UWSM path currently inherits
`/dev/null` for stdout and stderr. Client `eprintln!` diagnostics, a returned
`anyhow` error, and panic text can therefore disappear. A signal crash may leave
a system coredump, but orderly error exits and lifecycle races have no durable
postmortem evidence.

The 2026-08-09 terminate-Dojo/session-picker incident demonstrated the gap: the
scope exit and durable topology were available, but there was no client event
stream identifying whether the exit was a final-tab lifecycle decision or a
frontend error.

## Product contract

### Process and window scope

Diagnostics are initialized only for commands that can create a graphical
window. Parsing the CLI remains side-effect free, after which graphical command
classification and diagnostics initialization occur before configuration,
daemon connection, renderer configuration, or Wayland setup. Headless, relay,
machine-output, policy, and diagnostics commands do not create client log files
or install the graphical panic hook.

Each graphical process receives a random `client_instance_id`. Each mapped
window receives a random `window_id`; a process that sequentially maps more than
one window uses a new value for each mapping. These UUIDs are diagnostic
correlation identities, not Wayland object IDs. Events emitted before a window
is mapped omit `window_id`.

### Default sinks

A graphical client creates its log lazily at the first warning or error, when an
explicit debug/trace filter enables a diagnostic event, or when a non-clean
terminal lifecycle record must be written:

```text
$XDG_STATE_HOME/splinterm/logs/client-<UTC timestamp>-<pid>-<client_instance_id>.jsonl
```

Requirements:

- file mode `0600` and private parent directories;
- JSON Lines with a versioned, stable record schema;
- default level `warn`;
- warning and error records also sent to journald when its native socket is
  available;
- no correctness dependency on stdout or stderr;
- synchronous flush for the final error, panic, or graceful-signal record;
- at most 2 MiB per client log, including a reserved 16 KiB allowance for one
  terminal lifecycle record;
- at most 10 finalized retained client logs and at most 10 MiB total across
  finalized retained logs;
- clean logs without warnings removed during orderly teardown; and
- oldest-first pruning under a cross-process retention lock after securely
  opening the new log.

The writer opens files with exclusive creation and no symlink following, verifies
that the parent and file are owned by the current user, and holds a per-file
advisory lock for the process lifetime. Pruning skips files whose per-file lock
cannot be acquired, so it never removes an active writer. The strict count and
total-size limits apply to finalized retained logs; simultaneously active logs
are bounded individually but are not counted until their writers exit. The next
client startup or `splinterm diagnostics` prunes finalized files left behind by
a crashed process.

Normal event writes stop at `2 MiB - 16 KiB`. An oversized event is replaced by
a fixed `diagnostic_record_omitted` record. Exactly one terminal record may use
the reserved allowance. If even that fixed record cannot fit, the writer
truncates only the immediately preceding incomplete JSON line before appending
the terminal record; it never rotates into an unbounded second file.

`$XDG_RUNTIME_DIR/splinterm/` may hold lock or staging files, but any record that
must survive a normal error return is committed to `$XDG_STATE_HOME` and
synchronously flushed before return.

### Last-exit summary

Logs are not the authority for `--last-exit`, because an uneventful clean client
may never create a log. Every graphical client atomically replaces a separate
private, fixed-size summary during orderly teardown:

```text
$XDG_STATE_HOME/splinterm/last-client-exit.json
```

The summary contains only the terminal lifecycle `DiagnosticEvent`; it has mode
`0600`, uses the same typed schema, and is limited to one record. Panic and
graceful termination handling update it when the required operations are safe.
Fatal native signals do not update it in-process.

### Exit classification

When a logging sink exists, every graphical process produces exactly one terminal
lifecycle record. A process may map a standalone picker and then a live window;
such intermediate window teardown is a window lifecycle event, not a terminal
process record. The process retains the current or most recently mapped
`window_id` for its final record. An atomic first-terminal-record-wins guard
prevents duplicates between normal teardown, error unwinding, panic handling,
and graceful signal handling.

The closed `exit_class` values are:

```text
clean:user_close
clean:final_tab_removed
clean:session_picker_decision
clean:compositor_close
error:wayland_dispatch
error:topology_manager
error:pane_stream
panic
signal:termination
unknown
```

Classification is deterministic:

| Observed cause | Exit class |
| --- | --- |
| Splinterm-owned close command, shortcut, or confirmed UI action | `clean:user_close` |
| Managed removal of the final Dojo tab | `clean:final_tab_removed` |
| Standalone session picker selection or cancellation that concludes the graphical invocation | `clean:session_picker_decision` |
| Unsolicited `xdg_toplevel.close` with no pending Splinterm close action | `clean:compositor_close` |
| Wayland connect, global binding, dispatch, or teardown failure | `error:wayland_dispatch` |
| Topology manager failure or unexpected topology channel closure | `error:topology_manager` |
| Pane controller/update stream failure without a valid terminal exit notice | `error:pane_stream` |
| Panic caught by the installed graphical panic hook | `panic` |
| Gracefully handled SIGTERM or SIGINT | `signal:termination` |
| Orderly return whose cause cannot be mapped safely | `unknown` |

A standalone picker decision followed by a live-window launch does not commit a
terminal class. A normal pane process exit is not itself a client error. It
becomes `clean:final_tab_removed` only when managed topology removes the final
tab and concludes the graphical invocation; otherwise the window remains open
or follows its explicitly observed lifecycle cause.

Fatal signals such as SIGSEGV, SIGABRT, and SIGBUS cannot safely serialize JSON,
create files, prune retention, or flush general-purpose Rust writers. SIGKILL
cannot be handled at all. Splinterm therefore does not promise an in-process
terminal record for a fatal signal. `splinterm diagnostics --last-crash` may
report a synthetic `crash:signal_inferred` result from systemd coredump metadata;
that synthetic result is clearly labeled as external evidence and is not a
`DiagnosticEvent` claimed to have been written by the crashed client.

The terminal record includes the process ID, `client_instance_id`, `window_id`
when present, package version, build commit when available, active topology
revision when available, tab count, and safe typed correlation fields. It must
not include terminal-controlled titles or content. A bounded error chain may be
inspected in memory only to choose a closed error code; the chain is never
serialized.

All direct assignments to the Wayland scheduling exit boolean are replaced by
an idempotent `request_exit(ExitClass)` transition. Error precedence is encoded
in that transition: an error, panic, or graceful termination may replace a
pending clean cause until the terminal record is committed; after commitment no
cause can replace it.

### Correlation

Use stable field names shared by client and daemon:

- `schema_version`;
- `component` (`splinterm` or `splinterd`);
- `event`;
- `level`;
- `pid`;
- `client_instance_id` when known;
- `window_id` when present;
- `dojo_id` and `splint_id` only when needed for lifecycle correlation;
- `topology_revision`;
- `build_version` and `build_commit`;
- `exit_class`; and
- `error_code`.

UUID identities are acceptable correlation metadata. User-controlled names,
titles, paths, argv, and terminal bytes are not.

Graphical protocol requests that can trigger a daemon lifecycle decision carry
`client_instance_id` and `window_id`. The daemon includes those identifiers and
the resulting topology revision in its corresponding sanitized lifecycle event.
Window topology updates carry their observed topology revision into the
Wayland-owned state; terminal snapshot revision remains a distinct field and
must not be substituted for topology revision.

Daemon correlation work is part of this plan. It does not convert every existing
daemon trace into the client diagnostic schema. Instead, the bounded lifecycle
events consumed by `splinterm diagnostics` use the shared schema and never
forward the daemon's arbitrary `%error`, path, or peer-provided text fields.

Builds may inject `SPLINTERM_BUILD_COMMIT` at compile time from packaging or CI.
When it is absent, `build_commit` is null; runtime Git discovery is forbidden.

### Error privacy schema

Every diagnostic sink accepts only one pre-sanitized `DiagnosticEvent`; file,
journal, last-exit summary, stderr, and panic paths must not format their own
payloads. The event schema is an allowlist, not a denylist:

- `event`, `level`, `exit_class`, operation codes, protocol codes, and
  `error_code` are closed enums serialized to fixed static strings;
- `message` is selected from fixed static templates owned by Splinterm;
- numeric values, booleans, bounded counters, UUID identities, package versions,
  and compile-time source module/line metadata are typed fields;
- OS failures may include a numeric errno and a fixed operation code, but not the
  path, argv, peer-supplied text, or the operating system's free-form message;
- protocol failures may include a closed protocol error code, but not a raw
  request, response, frame, terminal payload, or peer-provided failure message;
- panic records contain a fixed `panic` event, thread category, and optional
  compile-time source location, but never the panic payload; and
- unknown errors collapse to a fixed `internal_error` code plus safe typed
  correlation fields.

It is forbidden to serialize or forward arbitrary `Display` or `Debug` output
from `anyhow::Error`, panic payloads, I/O errors, protocol objects,
user-controlled strings, or third-party errors. A bounded error chain may be
used only inside the process to classify an allowlisted code; it is never itself
a log field. Text redaction may be applied as defense in depth, but it is not the
privacy authority and cannot make an otherwise forbidden free-form field
acceptable.

The exact same serialized `DiagnosticEvent` payload is submitted to journald and
the private file. Journal transport metadata may surround the payload, but may
not add an independently formatted error or message. Tests must fail if a sink
API can accept an untyped string payload.

Best-effort stderr for direct graphical launches renders only the event's fixed
message template and safe typed codes. The top-level binary does not return
`anyhow::Result` through Rust's default `Termination` formatting path.

### Panic behavior

The graphical client installs a replacement panic hook rather than chaining to
the default hook. It emits one fixed-schema panic event, synchronously flushes
an already-open sink, and attempts the atomic last-exit summary update only with
operations supported by the normal panic context. It never reads or prints the
panic payload.

Replacing the formatting hook does not catch the panic or change the configured
unwind/abort behavior. Rust unwinding, process abort behavior, and system
coredump handling remain intact; preserving the default hook's payload rendering
is intentionally not part of the contract. If the sink was never opened, the
hook may securely create the fixed panic record, but failure to do so must not
recurse or replace the original panic.

### Debug controls

Support Rust-style module filtering without changing the safe event schema:

```bash
SPLINTERM_LOG=debug splinterm launch
SPLINTERM_LOG=wayland=trace,topology=debug,info splinterm launch
```

Debug and trace logs remain size-bounded. Input diagnostics record event classes,
physical keycodes, and state transitions only when explicitly enabled; they
still do not record composed text or clipboard data. Module filters decide which
predefined typed events are enabled; they do not enable arbitrary `tracing`
fields or formatted third-party events.

### User diagnostics

Add human-output commands:

```bash
splinterm diagnostics
splinterm diagnostics --last-exit
splinterm diagnostics --last-crash
```

`--last-exit` reads the one-record last-exit summary, not the newest retained
log. `--last-crash` reports the newest retained `panic` record or externally
inferred systemd coredump for the installed client executable, including the
evidence source. The default command reports:

- resolved installed client and daemon executable identities;
- package/build identities;
- user-service active state;
- the last-exit summary when present;
- the newest retained abnormal client log;
- matching sanitized daemon lifecycle events by correlation UUID; and
- coredump presence without dumping stack memory or environment metadata.

External `systemctl`, `journalctl`, and `coredumpctl` access is implemented behind
bounded adapters with timeouts and test fakes. Matching requires a correlation
UUID; time proximity alone is displayed only as uncorrelated nearby evidence.
Absent journald, coredump, build commit, or service-manager support is reported
as unavailable rather than treated as command failure.

Any future support bundle must be previewable and require explicit confirmation
before copying or publishing. This plan does not add upload behavior.

## Implementation outline

1. Define a small, binary-owned diagnostics module with the closed
   `DiagnosticEvent`, `DiagnosticErrorCode`, `ExitClass`, correlation context,
   and sink traits. Keep sink APIs unable to accept strings or arbitrary tracing
   fields.
2. Parse the CLI, classify graphical commands without side effects, then
   initialize graphical diagnostics before configuration, daemon connection,
   renderer setup, or Wayland setup. Replace `main() -> anyhow::Result<()>` with
   a wrapper that classifies top-level errors and emits fixed stderr text.
3. Implement the secure bounded JSONL writer, final-record reserve, per-file and
   retention locks, atomic last-exit summary, crash-leftover pruning, and
   journald adapter.
4. Add typed module filtering. Replace lifecycle-relevant `eprintln!` calls and
   returned graphical errors with structured events; retain only fixed-template
   typed stderr rendering for direct launches.
5. Add `client_instance_id` and `window_id`, propagate them through relevant
   graphical protocol requests, and add matching sanitized daemon lifecycle
   events. Carry topology revision in `WindowTopologyUpdate` and Wayland state.
6. Replace `scheduling.exit` writes with `request_exit(ExitClass)`, using the
   mapping and precedence rules above. Return or commit the selected class before
   Wayland application state is dropped.
7. Install the replacement panic hook and graceful SIGTERM/SIGINT handling.
   Do not install fatal-signal handlers that attempt general-purpose logging.
8. Add `splinterm diagnostics`, `--last-exit`, and `--last-crash` using injectable
   service, journal, and coredump adapters. Diagnostics itself performs retention
   cleanup but does not create a graphical client log.
9. Add schema/privacy, retention, lifecycle, subprocess, daemon-correlation, and
   adapter tests.

## Required validation

- unit tests for exclusive/no-follow creation, modes, ownership rejection,
  per-file bounds, terminal-record reserve, count/size retention, active-log
  skipping, concurrent pruning, clean-log removal, and atomic last-exit updates;
- table-driven unit tests mapping every observable exit path to every exit
  classification, including precedence and exactly-once commitment;
- panic-hook subprocess tests proving a retained bounded record, no panic payload
  in file/journal/stderr, and unchanged unwind or abort outcome;
- graceful SIGTERM/SIGINT subprocess tests, plus tests proving fatal crashes are
  reported only as externally inferred evidence;
- schema/privacy tests proving that no sink accepts arbitrary strings or error
  formatting, using sentinel terminal bytes, panic payloads, key text, clipboard
  text, argv, environment values, cwd, OS errors, protocol failures, and
  terminal-controlled titles;
- top-level returned-error tests proving arbitrary `anyhow` text does not reach a
  diagnostic sink or stderr;
- null-stdout/stderr subprocess tests that force a pre-map Wayland startup error
  and prove state diagnostics survive without mapping a real window;
- protocol and daemon tests proving correlation UUID and topology revision
  propagation without user-controlled names or raw daemon errors;
- adapter tests for journald available/unavailable, service manager
  available/unavailable, coredump present/absent, command timeout, malformed
  output, and uncorrelated nearby journal records;
- optional Linux/systemd integration tests for real journald and coredump
  discovery, excluded from the default workspace test run;
- `cargo fmt --all --check`;
- `cargo clippy --workspace --all-targets -- -D warnings`;
- `cargo test --workspace`; and
- `git diff --check`.

## Non-goals

- terminal session recording;
- telemetry or remote upload;
- unbounded debug traces;
- silently collecting support bundles;
- logging fatal native signals from an async-signal-unsafe in-process handler; or
- replacing `systemd-coredump` for native signal crashes.
