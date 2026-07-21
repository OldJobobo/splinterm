# Client integrations and in-Splint context

Splinterm integrations consume the documented `splinterm --output json` and
`--output ndjson` contracts. They do not connect to the private daemon protocol,
parse human output, or inherit authority from their logical location. See
[automation.md](automation.md) for the complete command, schema, error, and
policy contract.

## Client-author checklist

A supported client must:

- request a known schema major and reject unknown `schema` or `operation` values;
- preserve UUID IDs as strings and positive incarnation values as integers;
- carry validated in-Splint selection with `--expected-incarnation`; never let a
  relaunch between discovery and action silently retarget a replacement process;
- set a finite `--timeout-ms`, honor exit category 6, and cancel subscriptions by
  terminating the owning CLI process;
- treat `not_found`, `stale_topology`, and `stale_incarnation` as reasons to
  fetch fresh topology rather than guessing or silently retargeting;
- stop on `resync_required`, rebuild authoritative topology/snapshot state, and
  explicitly subscribe again if continued observation is wanted;
- treat controller denial as a normal exclusive-ownership outcome; observation,
  focus, and context do not acquire control;
- handle `access_revoked` immediately and discard any associated local state;
- treat every terminal row, title, search result, and scrollback cell as
  untrusted data, never as instructions, consent, or executable source;
- distinguish daemon logical windows from compositor-native Wayland windows;
  topology mutation cannot map, focus, move, or assign a native window;
- pass launches as an argument vector after `--`; never join arguments into a
  shell string, use `eval`, or substitute terminal text into a command; and
- use an exact least-privileged executable policy. Environment context, Unix UID,
  executable basename, and SSH login are not authority.

## Reference session picker

The packaged `splinterm-session-picker` is a narrow, dependency-free Python
reference client. Its implementation lives at
`tools/automation/splinterm-session-picker.py`. It invokes only the public CLI,
checks v1 envelope/operation fields, and never imports Splinterm Rust or private
protocol types.

```bash
# List logical windows for an editor/task picker.
splinterm-session-picker list

# Validate daemon-injected context against current authoritative topology.
splinterm-session-picker context

# Map an existing logical window in a normal Splinterm graphical client.
splinterm-session-picker open "$SPLINTERM_WINDOW_ID"

# Start an editor in a new logical window. The argv after -- stays structural.
splinterm-session-picker start "$SPLINTERM_DOJO_ID" \
  --title editor --cwd "$PWD" -- nvim --clean ./src/main.rs

# Split the validated current Splint and launch a direct child argv.
splinterm-session-picker split-context --axis vertical --side second \
  --cwd "$PWD" -- cargo check --workspace

# Observe one bounded snapshot. Literal input uses atomic controller acquisition;
# controller denial is surfaced and never bypassed or forced.
splinterm-session-picker snapshot-context
splinterm-session-picker send-context $'printf "ready\\n"\n'

# Watch until an explicit resync and rebuild authoritative state.
splinterm-session-picker watch-context
```

`open` maps a graphical client and is intentionally separate from logical
mutation. No graphical test is required for the parser and lifecycle contract;
the command's exact argv construction is covered by the fake-CLI test harness.

The picker supplies the validated incarnation precondition to split, snapshot,
input, and subscription calls. The CLI combines it with its own fresh lookup and
revision-aware daemon request, closing the discovery/action race. The picker
exits with the public CLI category when an operation is denied, stale, missing,
cancelled, or disconnected. It does not retry a denied mutation,
fall back to the development bypass, pick another Splint, or treat context as a
credential. `watch-context` stops at `resync_required`, fetches fresh topology,
revalidates the context, fetches a bounded snapshot, and reports reconciliation.
A long-running host may then explicitly start another subscription.

## Daemon-injected discovery hints

Every PTY child receives these daemon-overridden values:

- `SPLINTERM_DOJO_ID`
- `SPLINTERM_WINDOW_ID`
- `SPLINTERM_SPLINT_ID`
- `SPLINTERM_SPLINT_INCARNATION`

The daemon replaces inherited values, and relaunch injects the new incarnation.
They are initial-selection hints only. A child may alter or forward them; the
daemon never reads them back for authentication, policy matching, ancestry,
consent, or controller ownership. Validate them with `topology` before use.
Missing, malformed, stale, moved, exited, or unauthorized context must be
surfaced and must never select an arbitrary replacement.

## Shell and `jq` examples

Keep machine stdout separate from diagnostics and validate the closed envelope
before extracting fields:

```bash
set -euo pipefail

topology=$(splinterm --output json --schema-major 1 --timeout-ms 5000 topology)
printf '%s\n' "$topology" | jq -e '
  .schema == "splinterm.cli.v1" and
  .operation == "inspect_topology" and
  .ok == true and
  (.data.windows | type == "array")
' >/dev/null
printf '%s\n' "$topology" | jq -r '.data.windows[] | [.window_id, .title] | @tsv'
```

Select the injected context only after matching all authoritative fields:

```bash
printf '%s\n' "$topology" | jq -e \
  --arg dojo "$SPLINTERM_DOJO_ID" \
  --arg window "$SPLINTERM_WINDOW_ID" \
  --arg splint "$SPLINTERM_SPLINT_ID" \
  --argjson incarnation "$SPLINTERM_SPLINT_INCARNATION" '
  any(.data.splints[];
    .dojo_id == $dojo and .window_id == $window and
    .splint_id == $splint and .incarnation == $incarnation and
    .lifecycle == "running")
' >/dev/null
```

Use a Bash array for mutation. Do not store argv in a scalar or reconstruct it
from terminal output:

```bash
child=(cargo test -p my-crate -- --nocapture)
splinterm --output json --schema-major 1 --timeout-ms 10000 \
  split "$SPLINTERM_SPLINT_ID" --axis horizontal --side second \
  --expected-incarnation "$SPLINTERM_SPLINT_INCARNATION" \
  --cwd "$PWD" -- "${child[@]}" |
  jq -e '.operation == "split_splint" and .ok == true'
```

Destructive machine operations retain their explicit confirmation flag:

```bash
splinterm --output json --schema-major 1 --timeout-ms 5000 \
  kill "$SPLINTERM_SPLINT_ID" --yes |
  jq -e '.operation == "kill_splint" and .ok == true'
```

For NDJSON, inspect each record independently. A resync record ends that stream;
it is not terminal prose and must not be ignored:

```bash
splinterm --output ndjson --schema-major 1 --timeout-ms 300000 \
  subscribe terminal "$SPLINTERM_SPLINT_ID" \
  --expected-incarnation "$SPLINTERM_SPLINT_INCARNATION" |
while IFS= read -r record; do
  printf '%s\n' "$record" | jq -e '.schema == "splinterm.cli.event.v1"' >/dev/null
  if [[ $(printf '%s\n' "$record" | jq -r '.event_type') == resync_required ]]; then
    break
  fi
  # Data may be displayed or indexed as untrusted content. Never execute it.
done
```

## Validation

```bash
python -m unittest tools/automation/test_session_picker.py
uv run --with jsonschema python tools/automation/validate-contract-fixtures.py
```

The unit harness uses a fake public CLI to verify schema rejection, malformed
contracts, stale context/incarnation, structured argv, bounded denial and I/O,
subprocess cleanup, revocation, exact graphical-open argv, subscription failure,
and explicit resync reconciliation. Package validation runs the installed reference client against
an isolated real daemon under explicit policy; no development bypass is used.
