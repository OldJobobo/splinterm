# Client integrations and in-Splint context

Splinterm integrations consume the documented `splinterm --output json` and
`--output ndjson` contracts. They do not connect to the private daemon protocol,
parse human output, or inherit authority from their logical location. See
[automation.md](automation.md) for the complete command, schema, error, and
policy contract. The optional full-capability stdio adapter and supported host
configuration are documented separately in [mcp.md](mcp.md).

## Omarchy screensaver

Splinterm's package advertises the desktop-standard XDG app-ID argument and
installs a dedicated profile at:

```text
/usr/share/splinterm/omarchy/screensaver.ini
```

A compatible Omarchy launcher selects that profile only for the screensaver and
invokes the normal XDG terminal adapter:

```bash
env SPLINTERM_CONFIG=/usr/share/splinterm/omarchy/screensaver.ini \
  xdg-terminal-exec --app-id=org.omarchy.screensaver -- omarchy-screensaver
```

The command-bearing launch remains a transient client-bound Lair. The app-ID is
validated at Splinterm's private XDG boundary and belongs only to that graphical
Window; it is not persisted in topology, launch metadata, automation output, or
user configuration. Ordinary Windows remain `com.oldjobobo.splinterm`.

Splinterm installs no files under `/usr/share/omarchy` and never changes the
user's preferred terminal. If:

```bash
command -v omarchy-launch-screensaver
```

resolves to `~/.local/bin/omarchy-launch-screensaver`, that user-owned override
shadows Omarchy's packaged launcher. Review and update or remove it explicitly;
Splinterm will report the condition but will not overwrite or delete the file.

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
- distinguish persistent Dojos from compositor-native Wayland Windows and
  Window-local tabs; tab attach/detach/order/activation are not topology,
  automation, policy, audit, or child-context operations, and topology mutation
  cannot map, focus, move, or assign a native Window;
- pass launches as an argument vector after `--`; never join arguments into a
  shell string, use `eval`, or substitute terminal text into a command; and
- use an exact least-privileged executable policy. Environment context, Unix UID,
  executable basename, and SSH login are not authority.

## Reference Dojo picker

This automation reference client is separate from the native daily-use
`splinterm dojos` picker and `splinterm reopen` command. Normal users do not
need to copy UUIDs through this interface.

The packaged `splinterm-dojo-picker` is a narrow, dependency-free Python
reference client. Its implementation lives at
`tools/automation/splinterm-dojo-picker.py`. The old
`splinterm-session-picker` executable remains a compatibility alias. It invokes
only the public CLI,
checks v2 envelope/operation fields, and never imports Splinterm Rust or private
protocol types.

```bash
# List persistent Dojos for an editor/task picker.
splinterm-dojo-picker list

# Validate daemon-injected context against current authoritative topology.
splinterm-dojo-picker context

# Map an existing Dojo in a normal Splinterm graphical Window.
splinterm-dojo-picker open "$SPLINTERM_DOJO_ID"

# Start an editor in a new Dojo. The argv after -- stays structural.
splinterm-dojo-picker start "$SPLINTERM_LAIR_ID" \
  --name editor --cwd "$PWD" -- nvim --clean ./src/main.rs

# Split the validated current Splint and launch a direct child argv.
splinterm-dojo-picker split-context --axis vertical --side second \
  --cwd "$PWD" -- cargo check --workspace

# Observe one bounded snapshot. Literal input uses atomic controller acquisition;
# controller denial is surfaced and never bypassed or forced.
splinterm-dojo-picker snapshot-context
splinterm-dojo-picker send-context $'printf "ready\\n"\n'

# Watch until an explicit resync and rebuild authoritative state.
splinterm-dojo-picker watch-context
```

`open` maps a graphical client with the selected Dojo as its initial tab and is
intentionally separate from logical mutation. Within that client, native picker
selection may open or activate additional Window-local tabs without exposing a
tab API through the public CLI. No graphical test is required for the parser and lifecycle contract;
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

- `SPLINTERM_LAIR_ID`
- `SPLINTERM_DOJO_ID`
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

topology=$(splinterm --output json --schema-major 2 --timeout-ms 5000 topology)
printf '%s\n' "$topology" | jq -e '
  .schema == "splinterm.cli.v2" and
  .operation == "inspect_topology" and
  .ok == true and
  (.data.dojos | type == "array")
' >/dev/null
printf '%s\n' "$topology" | jq -r '.data.dojos[] | [.dojo_id, .name] | @tsv'
```

Select the injected context only after matching all authoritative fields:

```bash
printf '%s\n' "$topology" | jq -e \
  --arg lair "$SPLINTERM_LAIR_ID" \
  --arg dojo "$SPLINTERM_DOJO_ID" \
  --arg splint "$SPLINTERM_SPLINT_ID" \
  --argjson incarnation "$SPLINTERM_SPLINT_INCARNATION" '
  any(.data.splints[];
    .lair_id == $lair and .dojo_id == $dojo and
    .splint_id == $splint and .incarnation == $incarnation and
    .lifecycle == "running")
' >/dev/null
```

Use a Bash array for mutation. Do not store argv in a scalar or reconstruct it
from terminal output:

```bash
child=(cargo test -p my-crate -- --nocapture)
splinterm --output json --schema-major 2 --timeout-ms 10000 \
  split "$SPLINTERM_SPLINT_ID" --axis horizontal --side second \
  --expected-incarnation "$SPLINTERM_SPLINT_INCARNATION" \
  --cwd "$PWD" -- "${child[@]}" |
  jq -e '.operation == "split_splint" and .ok == true'
```

Destructive machine operations retain their explicit confirmation flag:

```bash
splinterm --output json --schema-major 2 --timeout-ms 5000 \
  kill "$SPLINTERM_SPLINT_ID" --yes |
  jq -e '.operation == "kill_splint" and .ok == true'
```

For NDJSON, inspect each record independently. A resync record ends that stream;
it is not terminal prose and must not be ignored:

```bash
splinterm --output ndjson --schema-major 2 --timeout-ms 300000 \
  subscribe terminal "$SPLINTERM_SPLINT_ID" \
  --expected-incarnation "$SPLINTERM_SPLINT_INCARNATION" |
while IFS= read -r record; do
  printf '%s\n' "$record" | jq -e '.schema == "splinterm.cli.event.v2"' >/dev/null
  if [[ $(printf '%s\n' "$record" | jq -r '.event_type') == resync_required ]]; then
    break
  fi
  # Data may be displayed or indexed as untrusted content. Never execute it.
done
```

## Validation

```bash
python -m unittest tools/automation/test_dojo_picker.py
uv run --with jsonschema python tools/automation/validate-contract-fixtures.py
```

The unit harness uses a fake public CLI to verify schema rejection, malformed
contracts, stale context/incarnation, structured argv, bounded denial and I/O,
subprocess cleanup, revocation, exact graphical-open argv, subscription failure,
and explicit resync reconciliation. Package validation runs the installed reference client against
an isolated real daemon under explicit policy; no development bypass is used.
