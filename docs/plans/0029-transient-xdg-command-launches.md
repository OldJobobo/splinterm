# Plan 0029: transient XDG command launches

- **Status:** Proposed hotfix
- **Date:** 2026-08-07
- **Product authority:** Splinterm is a standalone terminal emulator; Foot is its behavioral origin and oracle, not a runtime dependency or fallback
- **Integration authority:** `xdg-terminal-exec`, the installed Splinterm desktop entry, and Omarchy's command-bearing terminal launchers
- **Depends on:** accepted persistent topology, trusted installed-client identity, daemon process ownership, and connection cleanup

## Decision

Correct the XDG launch semantics so Splinterm behaves like a conventional default terminal when another application asks it to host a command:

```text
commandless XDG launch      -> persistent interactive Lair
command-bearing XDG launch  -> transient client-bound Lair
native splinterm launch     -> persistent Lair, with or without a command
sessions / reopen / window  -> existing persistent topology
```

The governing rule is:

> Opening a terminal creates persistent work. Asking the default terminal to
> host a command creates a transient application unless persistence is
> explicitly requested through Splinterm's native interface.

A transient XDG command must:

- preserve the exact working directory and structured argv without shell reconstruction;
- close its graphical Window and remove its complete Lair when the command exits;
- terminate its command and remove its complete Lair when the owning Window closes or its client process disconnects;
- disappear safely under a command-exit/owner-disconnect race;
- never be written into durable topology or restored after daemon restart;
- never appear in Recent Sessions or become the target of `reopen`;
- remain unavailable to automation as a way to request trusted client-owned lifecycle behavior; and
- leave commandless XDG launches and native Splinterm launches unchanged.

This is a Splinterm semantic correction. It must not route Omarchy, `xdg-terminal-exec`, or users through Foot or another terminal.

## Problem

Omarchy uses the configured default terminal for many short-lived TUI applications:

```text
omarchy-launch-tui
  -> xdg-terminal-exec --app-id=org.omarchy.<tool> -e <tool> <args...>
  -> com.oldjobobo.splinterm.desktop
  -> splinterm-xdg-terminal-exec
```

The installed wrapper currently sends every invocation through:

```text
splinterm launch --new --name terminal-<time>-<pid>
```

`launch` creates a durable Lair, remembers its Dojo as recent, drops the creation connection, and opens normal graphical attachment connections. Closing the Window therefore performs Splinterm's ordinary persistent detach instead of the conventional terminal behavior expected by command launchers. A TUI whose Window is closed can remain running in `splinterd`, and every such live Dojo appears in Recent Sessions.

Hiding these entries would reduce visible noise but would leave processes, PTYs, runtimes, and topology alive. The hotfix must correct ownership and cleanup, not only presentation.

## Product contract

### Interactive XDG launch

```bash
xdg-terminal-exec
xdg-terminal-exec --dir=/work
```

Both forms create an ordinary persistent Lair because no command was supplied. Closing the Window detaches it. The user may reopen it through Recent Sessions.

### Command-bearing XDG launch

```bash
xdg-terminal-exec -- btop
xdg-terminal-exec --dir=/work -- lazygit --debug
```

Both forms create a transient Lair owned by the exact trusted client connection that requested it. Command exit and owner disconnect converge on the same idempotent teardown.

### Native Splinterm launch

```bash
splinterm launch -- btop
splinterm new monitoring -- btop
```

These remain persistent. Command presence alone is not a global persistence heuristic; it selects transient behavior only inside the packaged XDG adapter path.

### Explicit XDG hold behavior

`--hold` is outside this hotfix because the current desktop entry does not advertise an `X-TerminalArgHold`. Do not silently reinterpret it. A later compatibility slice may define whether hold retains an exited Window without creating durable topology.

## Security and authority

Transient lifetime is trusted graphical-client behavior, not an automation scope.

Add a distinct private request such as:

```text
Request::CreateTransientLair {
    expected_topology_revision,
    name,
    launch,
}
```

Requirements:

- authorize it as `RequestAuthorization::TrustedUi`;
- require the installed matching Splinterm executable identity;
- require a nonempty direct command vector;
- provide no automation request variant;
- do not resolve `CreateLairAutomation` into it;
- do not allow persistent policy, development attach, or an SSH relay to claim its semantics; and
- increment the private protocol version.

Automation may continue to observe live topology when separately authorized, but it cannot create a transient owner lease or inherit trusted graphical authority.

## Design

### 1. Lair lifetime

Add an in-memory Lair-level lifetime:

```rust
pub enum LairLifetime {
    Persistent,
    Transient,
}
```

Missing lifetime data defaults to `Persistent`. `Lair::new` remains persistent; transient creation uses an explicit constructor or setter internal to the trusted path.

Lair-level ownership is deliberate. If the user creates another Dojo or split while the transient Window is open, the complete transient Lair remains client-bound and is removed together. Do not permit a transient child to become durable accidentally.

The lifetime identifies cleanup and projection policy. It does not contain the owning connection ID.

### 2. Durable projection

`TopologyDocument::from_topology` must omit every transient Lair on every save, not only during transient creation. Any later persistent rename, split, exit, or daemon shutdown may serialize the aggregate in-memory topology.

Compatibility rules:

- existing schema-v2 and schema-v3 documents decode every untagged Lair as persistent;
- persistent documents retain their existing representation where practical;
- new code never writes a transient Lair;
- if an explicitly tagged transient Lair is encountered in metadata, discard it instead of restoring or relaunching it; and
- recent-Dojo documents need no migration because absent IDs are already harmless.

No generated name, command name, app ID, or timestamp may be used to infer lifetime.

### 3. Owner-connection lease

Add an in-memory daemon registry with both lookup directions:

```text
owner connection ID -> transient Lair ID
transient Lair ID    -> owner connection ID
```

One XDG adapter connection owns one transient Lair. Connection IDs are never serialized into the core model, topology snapshots, metadata, public JSON, or audit bodies.

Creation must be cancellation-safe against owner disconnect, peer-process death, or direct abortion of the in-flight handler future:

1. Validate trusted identity, nonempty argv, bounds, name, and topology CAS.
2. Allocate the Lair/Dojo/Splint identities and reserve the owner lease before the first cancellable process-spawn await.
3. Spawn the runtime under a cleanup guard.
4. Commit runtime registration, in-memory topology, and lease ownership coherently under the topology transaction boundary.
5. Disarm the cleanup guard only after the complete commit is visible.
6. On handler-future cancellation or any failure, terminate/reap the runtime and remove every reservation without publishing topology.

`ClientFrame::Cancel` is not part of this hotfix's creation-cancellation contract: the daemon currently handles a request inline and cannot consume that frame concurrently. Adding concurrent protocol-request cancellation is a separate design change.

Transient insertion does not call `persist_topology`. Persistent creation keeps its existing durable path.

### 4. Kept-alive creation connection

The client currently drops its Lair-creation connection before starting the graphical Window. A connection-owned transient would therefore terminate immediately.

For transient XDG launch only:

- keep the exact successful creation connection alive in `app/sessions.rs` while `run_live_multipane_window(...).await` runs;
- do not transfer ownership to attachment, controller, topology, image, or focus connections;
- drop the owner connection after the Window returns on success or error; and
- do not modify `app/window.rs` merely to carry the lease.

This avoids collision with the active unrelated `window.rs` work and gives daemon `cleanup_connection` one authoritative termination signal.

### 5. Shared idempotent teardown

Implement one transient-Lair retirement operation used by both natural process exit and owner disconnect.

The operation must:

- serialize through `topology_transactions`;
- validate lifetime, owner identity where applicable, Splint identity, and incarnation;
- become a successful no-op when another teardown path already won;
- revoke grants and release controllers, transfers, subscriptions, graphical focus, and image/runtime resources using existing cleanup paths;
- request bounded runtime shutdown on owner disconnect;
- reap and remove the runtime entry;
- remove the complete transient Lair from in-memory topology;
- remove both lease indexes;
- publish the existing private `RuntimeChanged` topology kind with a snapshot and revision in which the Lair is absent; and
- never persist an exited or restorable transient state.

Reusing `RuntimeChanged` is intentional for this hotfix. `TopologyChangeKind` has an exhaustive public automation projection; adding `LairRemoved` would expand the frozen event vocabulary and require a separate public-schema decision.

Natural exit versus owner disconnect is an expected race. Whichever path obtains the transaction first owns cleanup; the loser must observe absence and finish without an internal error, duplicate publication, or leaked process.

Persistent process exit remains unchanged: it records `Exited`, persists durable launch metadata, and remains explicitly restorable.

For the hotfix, reuse the existing bounded runtime shutdown escalation. Do not widen timeouts or change PTY signal policy in this slice. The Window may close immediately while resistant-process reaping completes under the existing bound. A shorter transient-specific grace is a separate measured follow-up.

### 6. XDG-only client entry point

Do not parse or rebuild argument vectors in POSIX shell. Replace the wrapper's direct use of native `launch` with a hidden client subcommand, for example:

```text
splinterm xdg-launch [--working-directory PATH] [-- ARGV...]
```

The desktop wrapper remains a pure argv-preserving adapter:

```sh
exec splinterm xdg-launch "$@"
```

Client behavior:

- empty command vector -> ordinary persistent `CreateLair`;
- nonempty command vector -> trusted `CreateTransientLair`;
- reject the transient path for remote endpoints and machine output;
- native `splinterm launch -- ARGV...` remains persistent;
- preserve `--working-directory`, spaces, empty arguments, metacharacters, and the command delimiter exactly; and
- retain the existing daemon-start and incompatible-daemon restart behavior in the packaged wrapper.

The hidden subcommand is an integration boundary, not a public machine contract. The daemon remains the final authority and rejects transient creation from an untrusted client.

### 7. Recent Sessions exclusion

Use two defenses:

1. transient XDG launch never calls `remember_dojo`; and
2. `collect_sessions` skips transient Lairs before creating picker entries.

Because the standalone and in-Window Recent Sessions surfaces share the collector, both receive the same behavior. `reopen` must never resolve a transient Dojo, even if a stale recent file happens to contain its ID.

General authorized topology inspection may truthfully observe a live transient Lair during its short lifetime. This hotfix does not create connection-specific topology projections or expand the public JSON schema. Recent Sessions is the human persistence catalog; general topology remains an account of live daemon resources.

## Non-goals

- Route Omarchy or users through Foot, Alacritty, Kitty, Ghostty, or another terminal.
- Make Splinterm dependent on Foot at runtime.
- Infer transient behavior from `org.omarchy.*`, generated Lair names, executable names, titles, or environment variables.
- Make all command-bearing native Splinterm launches transient.
- Add a public automation operation for client-bound lifecycle.
- Change remote graphical semantics; remote Window disconnect continues to detach from persistent remote sessions.
- Add `--hold` support.
- Add a generic retention-policy configuration surface before the corrected default is validated.
- Automatically kill or delete existing untagged sessions that predate this hotfix.
- Change process-shutdown grace periods without separate evidence.
- Run graphical tests.

## Dependency-ordered implementation milestones

### Milestone 0 — preserve active work

Before editing, record `git status --short` and inspect every active diff. The worktree changed concurrently during planning and review as the remote graphical-client work continued, so no captured path list is authoritative for a later implementation session.

Treat every pre-existing modification or untracked file outside this plan as unrelated unless the user explicitly says otherwise. Do not overwrite, stage, revert, or assume those changes. The hotfix should avoid `window.rs` and `topology_manager.rs`; if implementation later proves either unavoidable, stop and reconcile scope and ownership before editing. Keep one writer in the active worktree.

### Milestone 1 — model and persistence contract

Files:

- `crates/splinterm-core/src/model.rs`
- `crates/splinterm-core/src/lib.rs`
- `crates/splinterm-core/src/persistence.rs`
- direct `Lair` test fixtures across the workspace

Implement `LairLifetime`, persistent defaults, explicit transient construction, and durable filtering.

Focused acceptance:

- old schema-v2 and schema-v3 fixtures load as persistent;
- persistent topology round-trips unchanged in meaning;
- a mixed topology encodes/restores only persistent Lairs;
- explicitly tagged transient metadata is not restored;
- global topology revision remains valid when filtered transient activity occurred; and
- no persistent Lair is dropped by the projection.

Stop if existing metadata compatibility regresses.

### Milestone 2 — private trusted creation contract

Files:

- `crates/splinterm-protocol/src/lib.rs`
- `crates/splinterd/src/authorization.rs`
- `crates/splinterd/src/audit.rs`
- `crates/splinterd/src/main.rs`
- `tools/package/validate-package.py`

Add `CreateTransientLair`, private protocol-version increment, exhaustive authorization/audit matching, and structured serialization tests. Reuse `TopologyChangeKind::RuntimeChanged` for removal. Update the package validator's pinned `PRIVATE_PROTOCOL_VERSION` in the same milestone so extracted-package relay checks cannot drift from the protocol crate.

Focused acceptance:

- exact cwd and argv round-trip;
- empty argv fails;
- installed trusted UI may request it;
- automation role, policy-authorized automation, relay identity, development attach, and mismatched client identity cannot request it; and
- existing `CreateLair` and `CreateLairAutomation` remain persistent.

Stop if any non-trusted path can create a transient lease.

### Milestone 3 — lease registration and transient creation

File:

- `crates/splinterd/src/main.rs`

Add the bidirectional lease registry, cancellation-safe reservation, transient create path, rollback guards, and resource-limit handling.

Focused acceptance:

- successful creation owns exactly one transient Lair;
- owner disconnect, peer death, or direct handler-future abortion during spawn leaves no lease, topology, runtime, or child PID;
- a later `ClientFrame::Cancel` is not misrepresented as supported concurrent cancellation;
- spawn and runtime-registry failures leave no partial state;
- transient insertion writes no metadata; and
- persistent creation still performs its existing durable commit and rollback.

Stop on any leaked PID, runtime, lease, or topology entry.

### Milestone 4 — exit and disconnect convergence

Files:

- `crates/splinterd/src/main.rs`
- `crates/splinterd/tests/end_to_end.rs`

Centralize transient retirement and connect it to process-exit observation plus `cleanup_connection`.

Focused acceptance:

- a transient `/bin/true` exits and disappears without an exited/restorable record;
- dropping the owner connection terminates a sleeping child and removes the Lair;
- dropping a non-owner attachment/controller/focus connection does not remove it;
- after adding a split or second Dojo to a transient Lair, owner disconnect reaps every runtime and removes the complete Lair;
- after adding a split or second Dojo, initial-command exit applies the documented whole-Lair policy and reaps every runtime;
- natural exit racing owner disconnect publishes one coherent `RuntimeChanged` snapshot with the Lair absent and leaks nothing;
- daemon restart never restores transient topology or command execution; and
- persistent command exit remains exited and restorable.

Use bounded deadlines and print remaining PID/topology/lease evidence on failure. Do not solve failures by widening deadlines.

### Milestone 5 — client and wrapper routing

Files:

- `dist/bin/splinterm-xdg-terminal-exec`
- `crates/splinterm/src/app/commands.rs`
- `crates/splinterm/src/app/cli.rs`
- `crates/splinterm/src/app/session_catalog.rs`
- `crates/splinterm/src/app/sessions.rs`
- `tools/package/validate-package.py`

Add hidden `xdg-launch`, route based on its parsed command vector, preserve the owner connection for transient Window lifetime, and keep native launch persistent.

Focused acceptance:

- wrapper with no arguments is persistent;
- wrapper with only a working directory is persistent;
- wrapper with `-- executable args...` is transient;
- native `splinterm launch -- executable args...` remains persistent;
- exact argv, empty arguments, spaces, and metacharacters survive without shell evaluation;
- transient launch is not recorded as recent;
- aliases `splinterm-sessions` and `splinterm-reopen` remain unchanged; and
- incompatible packaged client/daemon versions still trigger the existing restart path; and
- a non-graphical client unit test uses a pending stand-in Window future to prove the exact creation connection remains open until both successful and failed Window completion, then closes exactly once.

### Milestone 6 — picker defense

Files:

- `crates/splinterm/src/session_picker.rs`
- `crates/splinterm/src/app/sessions.rs`

Filter transient Lairs from the shared collector and prove `latest_reopenable` cannot return one.

Focused acceptance:

- mixed persistent/transient topology yields only persistent picker rows;
- a stale transient recent ID is harmless;
- standalone and in-Window Recent Sessions use the same filtered collector; and
- persistent ordering and recency behavior do not change.

### Milestone 7 — documentation and package hotfix

Files:

- `README.md`
- `site/src/content/docs/docs/quickstart.md`
- `site/src/content/docs/docs/sessions.md`
- `site/src/content/docs/docs/status.md`
- `docs/configuration.md`
- `docs/packaging.md`
- `packaging/PKGBUILD`

Document the corrected interactive-versus-command contract. Increment `pkgrel` for the hotfix package only when the implementation is validated and a release is actually being prepared.

Do not install the package, replace `/usr/bin/splinterm`, restart the user's production daemon, launch a Window, publish a release, or push changes without the separate approval required for those actions.

## Validation

Run focused checks after each coherent milestone, then the complete non-graphical suite:

```bash
sh -n dist/bin/splinterm-xdg-terminal-exec
cargo test -p splinterm-core
cargo test -p splinterm-protocol
cargo test -p splinterd --bin splinterd
cargo test -p splinterd --test end_to_end transient -- --test-threads=1
cargo test -p splinterm
cargo fmt --all --check
cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace
git diff --check

cd site
npm run validate
```

Extend `tools/package/validate-package.py::validate_launcher` to validate both persistent and transient wrapper routing against an extracted package. Update and assert the validator's pinned `PRIVATE_PROTOCOL_VERSION` in the protocol milestone, before wrapper assertions. Run the repository's normal package build/validation flow only after source validation is green.

No graphical test is required or authorized. Owner-connection loss, command exit, restart exclusion, and cleanup races are validated at the daemon/protocol boundary without mapping a Wayland Window.

## Required test matrix

| Case | Expected result |
| --- | --- |
| XDG, no command | Persistent, remembered, reopenable |
| XDG, cwd only | Persistent with exact cwd |
| XDG, command | Transient with exact cwd/argv |
| Native `launch`, command | Persistent |
| Transient command exits | Window shutdown signal, runtime reaped, Lair removed |
| Owner connection closes | Command terminated, runtime reaped, Lair removed |
| Non-owner connection closes | Transient command and Lair remain |
| Pending stand-in Window future | Creation connection stays alive through success and error, then closes once |
| Transient Lair gains split/second Dojo | Whole-Lair exit/disconnect cleanup reaps every runtime |
| Exit/disconnect race | One idempotent `RuntimeChanged` removal snapshot, no leak |
| Handler aborted during spawn | No process, runtime, lease, or topology residue |
| Daemon restart | Transient absent; persistent topology restored |
| Recent Sessions | Transient absent while live and after removal |
| Automation request | Transient creation denied |
| Spawn/create failure | No process, runtime, lease, or topology residue |
| Existing metadata | Untagged Lairs remain persistent |

## Migration and existing-session policy

The hotfix applies prospectively.

- Existing untagged Lairs remain persistent, even when their names look generated or their commands resemble Omarchy TUIs.
- Existing running processes are never killed automatically during upgrade.
- Existing exited sessions remain restorable.
- Existing recent files need no rewrite.
- Users may explicitly terminate or close old unwanted sessions after reviewing them.
- `splinterm reset` is not presented as routine cleanup because it affects every session.

This avoids destructive heuristics and preserves the meaning of all pre-hotfix data.

## Stop-loss boundaries

Stop implementation and report before continuing if:

- old metadata no longer loads as persistent;
- a transient Lair reaches durable metadata;
- automation or a nonmatching executable can request transient trusted lifecycle;
- owner disconnect, peer death, handler-future abortion, exit, or runtime-registry failure leaves a child PID or partial state;
- native `splinterm launch` changes from persistent to transient;
- structured cwd or argv is rebuilt through a shell;
- the hotfix overlaps any active unrelated worktree edit without first reconciling scope and writer ownership;
- validation would require graphical manipulation not already approved;
- package installation or replacement of Pacman-owned binaries becomes necessary; or
- publishing, pushing, or production daemon restart becomes necessary without explicit approval.

## Review requirements

Before completion, obtain one fresh product/compatibility review and one fresh daemon-lifecycle/security review at the coherent validated boundary.

The product review must verify:

- Splinterm remains the selected standalone default terminal;
- commandless XDG and native launch remain persistent;
- command-bearing XDG behavior matches conventional terminal expectations;
- no user-facing workaround routes through another terminal; and
- documentation describes the distinction without Foot dependency language.

The lifecycle review must verify:

- trusted-only creation;
- disconnect- and handler-abort-safe lease commit;
- idempotent exit/disconnect teardown using the existing public-compatible topology event vocabulary;
- complete resource cleanup;
- durable exclusion;
- migration safety; and
- no unresolved process or topology leaks.

## Completion criteria

This hotfix is complete only when:

- command-bearing `xdg-terminal-exec` launches use transient client-bound Lairs;
- commandless XDG launches remain persistent;
- native Splinterm launches remain persistent regardless of command presence;
- command exit and owner disconnect both remove transient topology and reap the process;
- transient Lairs are absent from durable metadata, restart restore, Recent Sessions, and `reopen`;
- automation cannot mint transient trusted-client semantics;
- exact cwd and argv transport is preserved;
- existing untagged sessions remain untouched;
- focused lifecycle, security, migration, wrapper, picker, full workspace, site, and package validations pass;
- recorded independent reviews have no unresolved blockers; and
- no graphical testing, production installation, publication, or unrelated-worktree modification occurred without separate approval.
