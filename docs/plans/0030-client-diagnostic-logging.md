# Plan 0030: bounded client diagnostic logging

- **Status:** Proposed
- **Date:** 2026-08-10
- **Scope:** graphical `splinterm` client diagnostics, daemon correlation, crash retention, and privacy boundaries
- **Related:** [Plan 0017](0017-inline-session-picker-overlay.md), [Plan 0019](0019-dojo-tabs.md), [Plan 0025](0025-command-palette-and-tab-context-menus.md)

## Decision

Add low-volume structured diagnostics to every graphical Splinterm client. The
normal sink is a private bounded per-process file, with warning and error events
also submitted to journald when available. Clean, uneventful logs are removed;
logs containing a warning, error, panic, or abnormal exit are retained under a
strict count and size budget.

The design follows the useful parts of Alacritty's on-demand per-process log,
WezTerm's runtime log directory and module filters, Foot's warning-level default,
and systemd's journal/coredump integration.

Diagnostic logging is not terminal recording. Splinterm must never log PTY
contents, key text, clipboard contents, command arguments, environment values,
OSC payloads, image bodies, or shell working directories by default.

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

### Default sinks

Each graphical client creates its log lazily at the first warning/error or at an
abnormal exit:

```text
$XDG_STATE_HOME/splinterm/logs/client-<UTC timestamp>-<pid>.jsonl
```

Requirements:

- file mode `0600` and private parent directories;
- JSON Lines or an equivalently machine-readable stable record format;
- default level `warn`;
- warning/error records also sent to journald when its native socket is
  available;
- no correctness dependency on stdout or stderr;
- synchronous flush for the final fatal/panic record;
- at most 2 MiB per client log;
- at most 10 retained client logs and at most 10 MiB total;
- oldest-first pruning after securely opening the new log; and
- clean logs without warnings removed during orderly teardown.

`$XDG_RUNTIME_DIR/splinterm/` may hold an active temporary file, but abnormal
records must be moved or copied into `$XDG_STATE_HOME` before a normal error
return so they survive logout/restart long enough for diagnosis.

### Exit classification

Every graphical client must produce exactly one terminal lifecycle record when
its logging sink exists:

```text
clean:user_close
clean:final_tab_removed
clean:session_picker_decision
clean:compositor_close
error:wayland_dispatch
error:topology_manager
error:pane_stream
panic
signal
unknown
```

The record includes the process ID, window ID, package version, build commit when
available, active topology revision, tab count, and a bounded error chain. It
must not include terminal-controlled titles or content.

### Correlation

Use stable field names shared by client and daemon:

- `component` (`splinterm` or `splinterd`);
- `event`;
- `level`;
- `pid`;
- `window_id` when present;
- `dojo_id` and `splint_id` only when needed for lifecycle correlation;
- `topology_revision`;
- `build_version` and `build_commit`;
- `exit_class`; and
- `error`, sanitized and bounded.

UUID identities are acceptable correlation metadata. User-controlled names,
titles, paths, argv, and terminal bytes are not.

### Error privacy schema

Every sink accepts only one pre-sanitized `DiagnosticEvent`; file, journal,
stderr, and panic paths must not format their own payloads. The event schema is
an allowlist, not a denylist:

- `event` and `error_code` are closed enums serialized to fixed static strings;
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
from `anyhow::Error`, panic payloads, I/O errors, protocol objects, user-controlled
strings, or third-party errors. A bounded error chain may be used only inside the
process to classify an allowlisted code; it is never itself a log field. Text
redaction may be applied as defense in depth, but it is not the privacy authority
and cannot make an otherwise forbidden free-form field acceptable.

The exact same sanitized record is submitted to journald and the private file.
Tests must fail if a sink API can accept an untyped string payload.

### Debug controls

Support Rust-style module filtering without changing the safe event schema:

```bash
SPLINTERM_LOG=debug splinterm
SPLINTERM_LOG=wayland=trace,topology=debug,info splinterm
```

Debug and trace logs remain size-bounded. Input diagnostics record event classes,
keycodes, and state transitions only when explicitly enabled; they still do not
record composed text or clipboard data.

### User diagnostics

Add:

```bash
splinterm diagnostics
splinterm diagnostics --last-exit
splinterm diagnostics --last-crash
```

The command reports installed client/daemon identity, service state, the latest
sanitized client exit, matching daemon journal entries, and coredump presence.
Any support bundle must be previewable and require explicit confirmation before
copying or publishing.

## Implementation outline

1. Add a small client diagnostics module built on `tracing` with a bounded file
   writer and optional journald layer.
2. Initialize it before configuration, daemon connection, or Wayland setup.
3. Define the closed `DiagnosticEvent`/`DiagnosticErrorCode` schema and require
   every sink to accept only that type; classify errors without forwarding their
   arbitrary `Display` or `Debug` representations.
4. Replace lifecycle-relevant `eprintln!` calls with structured events while
   retaining sanitized stderr as a best-effort human sink for direct CLI launches.
5. Carry an explicit exit classification through the Wayland event loop instead
   of setting only `scheduling.exit`.
6. Install a chained panic hook that writes one fixed-schema panic record without
   the panic payload and flushes synchronously. Preserve normal Rust panic and
   system-coredump behavior.
7. Add startup pruning and clean-exit deletion with tests using an isolated
   `XDG_STATE_HOME`.
8. Add the diagnostics CLI and schema/privacy tests.

## Required validation

- unit tests for size/count retention and clean-log removal;
- unit tests for every exit classification;
- panic-hook subprocess test proving a retained bounded record;
- schema/privacy tests proving that no sink accepts arbitrary strings or error
  formatting, using sentinel terminal bytes, panic payloads, key text, clipboard
  text, argv, environment values, cwd, OS errors, protocol failures, and
  terminal-controlled titles;
- direct-launch and desktop-launch tests proving diagnostics survive null
  stdout/stderr;
- journald-available and journald-unavailable fallbacks;
- `cargo fmt --all --check`;
- `cargo clippy --workspace --all-targets -- -D warnings`;
- `cargo test --workspace`; and
- `git diff --check`.

## Non-goals

- terminal session recording;
- telemetry or remote upload;
- unbounded debug traces;
- silently collecting support bundles; or
- replacing `systemd-coredump` for native signal crashes.
