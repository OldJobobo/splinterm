# Command-line reference

This page is the repository authority for the human command inventory and common
CLI workflows. The executable's `--help` is authoritative for exact flags in the
current build. The stable JSON/NDJSON schema, policy, limits, cancellation, and
exit contracts are owned by [Automation](automation.md).

## Invocation and output modes

```text
splinterm [OPTIONS] [COMMAND]
```

Global options accepted by command parsing are:

```text
--output human|json|ndjson
--schema-major MAJOR
--timeout-ms MILLISECONDS
--remote PROFILE
```

Human output is the default. `--output json` emits one versioned machine
document for supported one-shot operations. `--output ndjson` is reserved for
subscriptions. Human-only graphical and local-administration commands reject
machine-contract combinations at execution rather than pretending their output
is stable automation.

Machine stdout is reserved for protocol records; diagnostics go to stderr. Use
schema major 2 unless a later checked-in contract is explicitly documented.

## First human workflows

```bash
splinterm sessions                 # trusted recent-session picker
splinterm reopen                   # most recent still-running Dojo
splinterm list                     # active Lairs
splinterm list --all               # include exited-only history
splinterm launch                   # fresh graphical Lair
splinterm launch --splint-id ID    # attach an exact Splint
splinterm window --lair-id L --dojo-id D
```

`sessions`, `reopen`, `window`, and `launch` are graphical human workflows. A
Window is disposable and may hold multiple client-local Dojo tabs; these commands
do not make compositor operations part of the machine API.

## Topology inspection

| Command | Purpose |
| --- | --- |
| `ping` | Check daemon reachability. |
| `list [--all]` | Summarize active Lairs, optionally including complete exited history. |
| `topology` | Inspect reviewed metadata for every Lair, Dojo, and Splint. |
| `inspect SPLINT_ID` | Inspect one stable Splint and its lifecycle metadata. |
| `focus` | Machine-only narrow projection of the keyboard-focused graphical Splint ID and safe current working directory. |

Examples:

```bash
splinterm ping
splinterm topology
splinterm inspect SPLINT_ID
splinterm --output json topology
splinterm --output json focus
```

`focus` never exposes terminal content, commands, PIDs, titles, or private state
paths. With no focused Splinterm Window, its two projection fields are null.

## Creation and layout mutation

| Command | Purpose |
| --- | --- |
| `new NAME [--cwd DIR] [-- ARGV...]` | Create a persistent Lair, one Dojo, and one live Splint. |
| `new-dojo LAIR_ID [--name NAME] [--cwd DIR] [-- ARGV...]` | Add a persistent Dojo with one live Splint. |
| `preset run NAME [--cwd DIR]` | Atomically add one complete configured Dojo preset to the exact invoking/focused Lair. |
| `split TARGET_SPLINT_ID --axis horizontal|vertical --side first|second [--ratio N] [--cwd DIR] [-- ARGV...]` | Split a leaf and launch its sibling. |
| `ratio TARGET_SPLINT_ID RATIO` | Set the selected leaf's parent split ratio in thousandths. |
| `rename-lair LAIR_ID NAME` | Rename a Lair. |
| `rename-dojo DOJO_ID NAME` | Rename a Dojo. |
| `rename-splint SPLINT_ID TITLE` | Rename a Splint title. |
| `dojo-focus-hint DOJO_ID SPLINT_ID` | Persist a presentation hint; it does not focus a client or Window. |

`ARGV` after `--` is executed directly, never through a shell. With no explicit
argv, creation uses the configured shell.

```bash
splinterm new project --cwd "$HOME/src/project"
splinterm new build -- /usr/bin/ninja -C /tmp/build
splinterm new-dojo LAIR_ID --name logs -- /usr/bin/journalctl -f
splinterm preset run personal-review --cwd "$HOME/src/project"
splinterm split SPLINT_ID --axis vertical --side second --ratio 600
splinterm ratio SPLINT_ID 500
```

Non-dry preset runs are private trusted-local human operations. The client
compiles direct argv locally, verifies exact managed-Splint context, and asks the
daemon to persist the complete tree in one revision before any process launch.
Use `--dry-run` for side-effect-free inspection. See [Dojo presets](presets.md).

The ratio range is 1–999; it is the share assigned to the first child.
In-Splint automation should add `--expected-incarnation N` where offered so a
concurrent relaunch cannot silently retarget the operation.

## Process and topology lifecycle

| Command | Purpose |
| --- | --- |
| `kill SPLINT_ID` | End the live process while retaining its Splint leaf. |
| `restore SPLINT_ID` | Start one exited Splint from saved launch metadata. |
| `restore-dojo DOJO_ID` | Restore every exited Splint in a saved Dojo. |
| `restore-lair LAIR_ID` | Restore every exited Splint in a saved Lair. |
| `relaunch SPLINT_ID [--cwd DIR] [-- ARGV...]` | Relaunch an exited Splint as a new incarnation. |
| `close SPLINT_ID` | Remove an exited leaf and collapse its parent branch. |
| `close-dojo DOJO_ID` | Remove a Dojo only when all its Splints have exited. |
| `reset` | Stop the daemon, move all session state to a backup, and restart cleanly. |

Human `kill` and `reset` prompt unless `--yes` is supplied. Human `close` and
`close-dojo` remove only already-exited topology and do not add another
interactive prompt. Machine `kill`, `close`, and `close-dojo` require `--yes`;
`reset` is local human administration and has no stable machine output contract.
Restore operations are explicit—saved argv is never executed automatically.

A stable Splint ID names the persistent leaf. Every process start increments its
positive **incarnation**. Machine clients use incarnation preconditions to reject
stale targets rather than acting on a replacement process.

## Terminal observation and input

| Command | Purpose |
| --- | --- |
| `snapshot SPLINT_ID` | Read one bounded live semantic terminal snapshot. Development mode only. |
| `scrollback SPLINT_ID [--max-rows N] [--cursor CURSOR]` | Read one bounded history page. |
| `search SPLINT_ID QUERY [--case-sensitive] [--max-results N] [--cursor CURSOR]` | Search history without echoing the query in machine output. |
| `send SPLINT_ID TEXT` | Send literal UTF-8 through an atomic controller workflow. Development mode only. |
| `resize SPLINT_ID COLUMNS ROWS` | Resize through an atomic controller workflow. Development mode only. |

```bash
splinterm snapshot SPLINT_ID
splinterm scrollback SPLINT_ID --max-rows 32
splinterm search SPLINT_ID 'failed' --max-results 20
splinterm send SPLINT_ID $'printf "ready\\n"\n'
splinterm resize SPLINT_ID 120 40
```

Terminal reads carry exact Splint, incarnation, terminal revision, and history
generation. Continuation cursors are opaque. Public machine records contain
semantic Unicode cells, not raw daemon frames. Input and search bodies are never
copied into bounded audit metadata.

## Subscriptions

Subscriptions require NDJSON:

```bash
splinterm subscribe terminal SPLINT_ID --output ndjson
splinterm subscribe topology --output ndjson
splinterm subscribe control SPLINT_ID --output ndjson
```

Terminal and control subscriptions accept an expected-incarnation precondition.
Each stream begins with a current-state record. Sequence gaps, replaced history,
or a stalled client emit a bounded `resync_required` record and terminate; the
caller must explicitly resubscribe.

## Authorization, policy, and audit

| Command | Purpose |
| --- | --- |
| `authorization status SPLINT_ID` | Inspect effective authority for one Splint. |
| `authorization revoke GRANT_ID` | Revoke an ephemeral grant. |
| `policy validate PATH` | Validate a policy through the daemon's secure loader without publishing it. |
| `policy inspect PATH` | Print normalized validated policy. |
| `policy reload` | Request reload through the canonical user service. |
| `audit [--after ID] [--max-records N]` | Read bounded daemon-lifetime body-free audit metadata. |

Persistent policy commands are local administration rather than a remote machine
surface. A missing policy grants no third-party persistent authority. Policy
reload is atomic, disconnects automation-role clients, and fails closed to a new
deny-all generation if the configured document is rejected. Read
[Automation](automation.md) and [Headless operation](headless.md) before changing
policy.

## Configuration, keymaps, and remotes

Local inspection does not contact the daemon:

```bash
splinterm config check
splinterm keymap list
splinterm keymap show
splinterm keymap show splinterm
splinterm keymap conflicts
splinterm remote list
splinterm remote inspect PROFILE
```

`remote check PROFILE` additionally starts bounded SSH/relay/daemon read-only
probes but does not map a Window. `--remote PROFILE` binds compatible graphical
or CLI operations to that configured endpoint:

```bash
splinterm remote check server
splinterm --remote server sessions
splinterm --remote server --output json topology
```

See [Configuration](configuration.md) for strict INI/keymap/profile schemas and
[Remote access](remote.md) for authentication and authority boundaries.

## Relay and service-facing commands

`relay --stdio` is the byte-transparent policy-scoped SSH automation transport.
`relay --graphical-stdio` is the bounded private graphical multiplexer. They are
fixed transport entry points, not interactive shell commands and not public JSON
schemas. `splinterd` itself exposes no network listener.

Service startup, persistence, backups, upgrades, and recovery are documented in
[Headless operation](headless.md) and [Packaging](packaging.md).

## Machine-operation summary

The supported schema-major-2 one-shot operation inventory is frozen in
[Automation](automation.md#one-shot-command-and-operation-inventory). Important
rules:

- JSON one-shots emit exactly one envelope; NDJSON is subscription-only.
- Syntax errors produce no machine stdout and exit 2.
- Parsed failures use stable categories: authorization 3, connection/schema 4,
  invalid/stale/resource 5, timeout/cancellation 6, unexpected internal 70.
- Machine mode never prompts; required confirmations use explicit `--yes`.
- Raw private protocol versions, request IDs, controller IDs, and Rust DTOs are
  not compatibility promises.
- `--remote` changes transport, not the public schema or authority required.

Checked-in public schemas live under [`dist/schemas/v2/`](../dist/schemas/v2/).
