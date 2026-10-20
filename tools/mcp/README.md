# MCP demo harness

Plan 0031's harness for demonstrating and accepting Splinterm MCP automation.
Milestone 1 provides the shared stdio host driver and the **non-graphical
preflight**. The graphical lane is not implemented yet; it will require its own
explicit approval under `AGENTS.md` Section 3 and runs only in the Omarchy VM.

Standard-library Python 3.11+ only. No Wayland, compositor, window, or VM
action happens anywhere in this directory's M1 code.

## Preflight

```bash
# A staged build: sibling splinterd, splinterm, and splinterm-mcp binaries.
cargo build --frozen --bin splinterd --bin splinterm --bin splinterm-mcp
python3 tools/mcp/run-demo.py preflight --staged-bin-dir target/debug

# The installed package set (default /usr/bin); nothing is installed or changed.
python3 tools/mcp/run-demo.py preflight --installed

# Retain evidence in an existing directory.
python3 tools/mcp/run-demo.py preflight --staged-bin-dir target/debug --evidence-dir /tmp/mcp-evidence
```

The command prints one JSON summary line. Exit status: `0` passed, `1`
preflight_failed, `2` cleanup_incomplete, `3` aborted, `64` usage error.

What it does, in order:

1. **Identity.** Resolves the three binaries, hashes each through one
   descriptor, and records canonical path, device/inode, size, mode, and
   SHA-256. It requires regular, executable, non-world-writable files owned by
   root or the caller, all in one directory. `splinterd` and `splinterm` must
   report the same sealed build pair (the same report the package provenance
   check uses), and `splinterm-mcp`'s `serverInfo.version` must match that
   pair's version. Installed mode also rejects a `PATH` entry that shadows the
   selected set and checks the package provenance record when one exists.
   Staged mode records the source commit and dirty state, refuses `/usr/bin`,
   and never falls back to installed binaries.
2. **Isolation.** Creates a nonce-named mode-0700 root under `$TMPDIR` with
   private HOME, XDG config/state/cache/data/runtime, TMPDIR, and socket. The
   child environment is built from scratch: no Wayland/X11/D-Bus session, no
   `SPLINTERM_POLICY` (the daemon starts deny-all), no Splint discovery hints,
   and no `SPLINTERM_ENABLE_DEV_ATTACH`.
3. **Daemon.** Starts the selected `splinterd` in its own session, waits until
   the private socket accepts connections, and checks the running process's
   executable device/inode against the selection.
4. **Protocol.** Initializes MCP `2025-11-25` exactly, sends
   `notifications/initialized`, records the whole catalog (34 tools today), and
   validates only the 11 tools the demo needs: name, wire input/output schema
   (against `dist/schemas/mcp/v2`), and annotations. Additions never fail the
   preflight; a missing or changed required tool does.
5. **Deny-all.** `splinterm.ping` must succeed, and
   `splinterm.inspect_topology` must fail with the structured `unauthorized`
   envelope and no data. A generic or `internal` error is not deny proof. No
   other tool is called, including `splinterm.report_agent_status` (a mutation).
6. **Negative cases.** Each runs in a fresh adapter: unsupported version,
   malformed capabilities, malformed JSON, batch array, oversized line, and
   unterminated frame must end the adapter without further messages; discovery
   before `notifications/initialized` and a duplicate `initialize` must be
   rejected with `-32600` while the session stays usable.
7. **Cleanup.** Closes every adapter's stdin and awaits exit; stops the daemon
   with SIGTERM; escalates only against exact owned PIDs (and recorded
   descendant `(pid, starttime)` pairs) after the grace period; verifies the
   socket is gone; checks that no process still names the private root;
   verifies the user's normal daemon socket, topology, and policy files are
   unchanged (equality only, never contents); and removes only the nonce root
   after checking its device/inode. It never kills by name, title, or user.

## Evidence

Without `--evidence-dir`, nothing is written: the summary line is the record.
With it, the run writes `mcp-demo-<UTC>-<nonce>/` (mode 0700, files 0600):

| File | Contents |
|---|---|
| `manifest.json` | lane, mode, nonce, file list, and files not applicable to preflight |
| `state.json` | state transitions with timestamps and owned PIDs, rewritten after each one |
| `report.json` | result, checks, negative cases, catalog names, build identity |
| `summary.md` | human-readable tables |
| `identities.json` | binary identities |
| `mcp-events.jsonl` | per-message metadata only: direction, kind, method, ID, outcome codes, sizes |
| `cleanup.json` | ordered cleanup attestation |
| `SHA256SUMS` | digests of the above |

Evidence never contains request or response bodies, terminal text, controller
or transfer handles, capability frames, policy bodies, argv, or environment
values. Every report field is a fixed vocabulary or a validated bounded value.

Preflight states: `created → daemon_ready → preflight_passed → cleaning →
clean`. `preflight_passed` is recorded only after the daemon and protocol
checks, never before. A run whose cleanup cannot be proven stays in `cleaning`
with result `cleanup_incomplete`.

## Driver

`mcp_host.McpHost` owns one adapter process: newline-delimited JSON-RPC,
monotonic IDs, response matching that retains notifications (bounded, with
explicit overflow), per-request and per-write deadlines, cancellation, strict
UTF-8/JSON/envelope checks, concurrent bounded stdout/stderr draining, EOF
distinct from timeout, and stdin-close then SIGTERM then SIGKILL cleanup of
the exact child. Its event sink receives allowlisted metadata only.

`tools/package/validate-mcp-package.py` still carries its own host; moving it
onto this driver is a follow-up once parity with its tests is shown.

## Tests

```bash
python3 -m unittest discover -s tools/mcp/tests -p 'test_*.py' -v
```

The tests use `tests/fake_adapter.py` and `tests/fake_daemon.py` to exercise
framing, ordering, deadlines, EOF, malformed output, forced escalation, and
every partial-start and cleanup boundary. They are not a substitute for the
real staged preflight, which CI runs in the `mcp-tests` job.
