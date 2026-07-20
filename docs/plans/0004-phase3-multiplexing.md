# Plan 0004: persistent multiplexing

- **Status:** Planned
- **Roadmap:** Phase 3 — Multiplexing
- **Foundation:** [Plan 0001](0001-terminal-kernel.md), [Plan 0002](0002-omarchy-terminal-mvp.md), [ADR 0001](../adr/0001-foot-rust-port.md)
- **Reference source:** Foot 1.27.0, commit `3c5b584b0eafa772eb4376fb6eaf6643399e190e`

## Goal

Turn the single-Splint MVP into a persistent local multiplexer without weakening
the daemon/client, authorization, or Foot-parity boundaries:

1. one daemon owns multiple independently live Splints;
2. each Dojo can contain multiple windows and each window a binary Splint tree;
3. disposable clients can select, detach, reattach, and concurrently observe the
   same topology;
4. input and size ownership remain exclusive, explicit, visible, and scoped to
   one Splint; and
5. durable metadata can be restored or relaunched honestly after daemon loss,
   without claiming PTY or process resurrection.

The first implementation slice is headless. It establishes stable multi-Splint
identity, a runtime registry, targeted lifecycle operations, and per-Splint
controller arbitration before any graphical tree UI is added.

## Current-state constraints

- `splinterm-core` already models stable Dojo, window, and Splint IDs and a binary
  `LayoutNode`, but exposes only creation, lookup, state change, and Dojo removal.
  It has no topology revision or transactional mutation API.
- `splinterd` stores `Mutex<Option<LiveSplintRuntime>>`; `InspectLiveSplint`,
  creation, lookup, termination, and shutdown all assume exactly one process.
- Controller state contains one global optional lease. Two clients cannot control
  different Splints even though terminal subscriptions are already multi-client.
- Most terminal requests already carry Splint ID and process incarnation. The
  unscoped `InspectLiveSplint` lookup and client `live_identity()` helper are the
  principal identity shortcuts that must be removed.
- The Wayland client has one `TerminalSnapshot`, one viewport, one selection,
  and one controller flag embedded in a large `App`. Reusing that state per pane
  requires an extraction boundary before split rendering.
- Scrollback has stable row IDs, generations, revision-bound paging, and bounded
  client caching. Search should extend these contracts rather than copy all
  history into the graphical client.
- The serialized core model is not currently written to disk. “Restore” must not
  be claimed until an owner-only, bounded, versioned, atomic metadata store exists.

## Product decisions fixed by this plan

1. **Identity:** Dojo, window, and Splint IDs remain stable and independent of
   names and tree positions. Every live process also has a nonzero incarnation;
   relaunch retains the Splint ID and allocates a new incarnation.
2. **Revisions:** topology has one monotonic `TopologyRevision`, separate from
   each terminal revision and history generation. Structural and naming changes
   advance topology exactly once per committed transaction.
3. **Focus:** keyboard focus, active pane, detached scroll position, selection,
   and search cursor are client-local. The daemon may persist a default active
   Splint hint, but one client must never move another client's focus.
4. **Control:** there is at most one controller/size owner per live Splint.
   Different Splints may be controlled simultaneously. Observation never implies
   control, and attaching never steals a lease.
5. **Kill versus close:** `KillSplint` ends the current process but retains an
   exited leaf and launch metadata. `CloseSplint` removes an exited leaf and
   collapses its parent; removing a live leaf requires an explicit kill-and-close
   operation. Closing the final leaf removes its window, not its Dojo.
6. **Restore:** during Phase 3, restore means loading validated topology and
   launch metadata, then explicitly relaunching selected exited Splints. It does
   not restore process memory, kernel PTYs, exact terminal state, or unpersisted
   scrollback.
7. **Search:** literal UTF-8 search is daemon-owned, bounded, authorized by
   `Scrollback`, and correlated by Splint/incarnation/revision/generation.
   Regex, fuzzy search, and cross-Splint indexing are deferred.
8. **Windows:** a Dojo window is a daemon-owned layout resource. The first UI
   implementation may use one Wayland toplevel/client process per selected
   `WindowId`; one process managing many Wayland toplevels is not required.
9. **Compatibility:** Phase 3 uses a clean protocol-version bump. No bridge to a
   running Phase 2 daemon is required; package upgrade may perform the existing
   bounded daemon restart with its documented process-loss warning.

## Non-goals

- supported third-party automation, stable public JSON/NDJSON, MCP, SSH relay,
  or a daemon network listener (Phase 4);
- transparent process continuity across daemon crash, upgrade, logout, or reboot;
- durable full terminal grids or scrollback bodies unless separately designed;
- collaborative simultaneous typing, shared client focus, or automatic control
  stealing;
- tabs, floating panes, arbitrary graph layouts, synchronized input, broadcast
  panes, regex search, or tmux configuration compatibility;
- changes to the pinned Foot oracle, broad parity tolerances, or renderer stack.

## Architectural invariants

- `splinterd` remains the only writer of topology, runtime, and durable metadata.
- `splinterm-core` remains independent of Tokio, PTYs, Wayland, and wire DTOs.
- one logical actor remains the serialization point for each live Splint; a
  topology coordinator serializes structural transactions without blocking PTY
  consumption.
- no daemon state lock is held across PTY spawn, actor request, process shutdown,
  filesystem I/O, consent UI, or protocol write.
- a failed spawn cannot leave an addressable phantom leaf; a failed persistence
  write cannot be reported as a committed durable mutation.
- stale Splint IDs, incarnations, topology revisions, controller IDs, search
  cursors, and subscriptions fail explicitly rather than selecting a fallback.
- bounded client queues and `ResyncRequired` remain mandatory per subscription;
  a slow observer cannot stall any PTY or topology mutation.
- grants and leases bind the exact Splint/incarnation. Relaunch, kill, close, and
  restore revoke old authority and release old control.

## Protocol and domain direction

Introduce explicit DTOs rather than exposing daemon runtime structs:

- `TopologySnapshot { revision, lair, runtimes }`, where runtime summaries carry
  Splint ID, optional live incarnation, lifecycle state, and exit status;
- targeted inspection (`InspectSplint { splint_id }`) instead of
  `InspectLiveSplint`;
- bounded launch parameters shared by `CreateDojo`, `SplitSplint`, `NewWindow`,
  and `RelaunchSplint`;
- structural requests carrying expected topology revision to reject lost updates;
- a bounded topology subscription with snapshot, ordered change, and resync
  events independent of terminal subscriptions;
- separate rename, kill, close, kill-and-close, and relaunch requests;
- per-Splint control status and explicit takeover request/decision events; and
- revision/generation-bound search pages with opaque bounded cursors.

Names are trimmed, nonempty UTF-8, bounded to 128 bytes, and unique only where
human selection would be ambiguous: Dojo names are Lair-unique; window and
Splint titles need not be globally unique. IDs are always accepted for exact
selection. Split ratios use a validated fixed integer unit (for example
1..=999 of 1000), not serialized `f32`; loading legacy `f32` metadata is not
needed because no durable format has shipped.

## Dependency-ordered implementation slices

### Slice 0 — contracts and adversarial model tests

**Work**

- Record controller granularity, topology revision, kill/close/relaunch, and
  restore semantics in an ADR before implementing wire types.
- Add pure `splinterm-core` mutation tests for split insertion, branch collapse,
  final-window behavior, stable IDs, ratio bounds, duplicate names, and failed
  mutation rollback.
- Add a versioned persistence schema fixture and reject unknown schema versions,
  duplicate IDs, invalid trees, unsafe paths, oversized collections, and running
  states that cannot survive daemon loss.

**Likely files:** `docs/adr/0006-multiplexing-lifecycle.md`,
`crates/splinterm-core/src/{layout,model}.rs`, new core test modules.

**Gate:** no daemon or UI changes until the pure model can express every required
transaction without exposing partially mutated trees.

### Slice 1 — multi-Splint daemon/protocol identity and lifecycle

This is the first code slice.

**Work**

1. Replace `Option<LiveSplintRuntime>` with a bounded registry keyed by
   `SplintId`. Store cloneable handles for fast lookup and retain exactly one
   owned runtime join/shutdown path per live incarnation.
2. Replace global controller state with maps indexed by Splint identity and
   controller ID. Keep connection ownership checks; initially one connection may
   own one controller, while separate connections can control separate Splints.
3. Add explicit topology/runtime inspection and make all client commands select
   a Splint ID. Remove first-live fallback behavior from daemon and client.
4. Add headless `SplitSplint`, `KillSplint`, and `RelaunchSplint` operations.
   Split creates a sibling leaf around a target leaf with explicit axis, side,
   ratio, cwd, argv/shell policy, and scrollback budget. Spawn and insertion are
   one reported transaction with cleanup on either-side failure.
5. Update daemon shutdown to drain every registry entry concurrently under a
   bounded deadline, report individual failures, and remove the socket only
   after all owned runtime shutdown paths settle.
6. Extend the CLI with explicit IDs for inspection, attach/snapshot, send,
   resize, kill, and relaunch. Human-friendly name selection waits for Slice 3.

**Likely files:** `crates/splinterm-core/src/{layout,model}.rs`,
`crates/splinterm-protocol/src/lib.rs`, `crates/splinterd/src/{main,live}.rs`,
`crates/splinterd/tests/end_to_end.rs`, `crates/splinterm/src/main.rs`, README.

**Focused tests**

- two Splints run different commands and preserve independent output, dimensions,
  revisions, history, incarnations, exit state, and controller leases;
- concurrent observers attach to both while a stalled subscriber on one cannot
  delay the other;
- killing one leaves the sibling, daemon, and sibling controller alive;
- relaunch retains Splint ID, changes incarnation, revokes old grants/lease, and
  rejects stale requests;
- failed second spawn leaves the original topology/runtime unchanged;
- daemon shutdown reaps every child and removes the socket.

**Exit gate:** the isolated end-to-end test creates one Dojo with two live
Splints, independently drives and resizes both, detaches/reattaches, kills and
relaunches one, proves stale-incarnation rejection, then cleanly reaps both.
No graphical code is required.

### Slice 2 — authoritative topology stream and complete tree editing

- Add topology revision compare-and-swap to split, close, resize-ratio, new/close
  window, and rename operations.
- Add bounded topology snapshot-plus-subscribe, ordered change events, overflow
  resync, and cancellation. Terminal damage stays on per-Splint subscriptions.
- Introduce a daemon topology coordinator so structural edits, runtime registry
  commits, exit-state changes, control release, grant revocation, and persistence
  scheduling have one observable order.
- Add model/property tests using deterministic random edit sequences. After each
  operation assert unique IDs, reachable runtime entries, valid binary trees,
  ratio bounds, and no empty retained windows.

**Gate:** two protocol clients race edits from the same base revision; exactly one
commits and the loser receives a stale-topology error plus current revision.

### Slice 3 — durable metadata, detach/reattach, rename, close, and restore

- Store a bounded schema-versioned Lair document under
  `$XDG_STATE_HOME/splinterm/` (with a documented fallback), using owner-only
  directory/file checks, no symlink following, write-temp/fsync/rename/fsync-dir,
  and startup quarantine of invalid data.
- Persist IDs, names, tree shape/ratios, cwd, direct argv or shell launch policy,
  scrollback budget, last dimensions, exit metadata, and opt-in relaunch intent.
  Never persist grants, controller tokens, PTY descriptors, terminal bodies,
  clipboard data, or live-process claims.
- On startup load every leaf as exited/restorable. Provide explicit restore-one,
  restore-window, and restore-dojo actions with per-leaf results; do not silently
  run commands merely because metadata exists.
- Add a session chooser to `launch`/`window` with exact ID selection and clear
  create-versus-attach behavior. Closing a Wayland window remains detach only.
- Add rename/kill/close/relaunch CLI workflows with confirmation for destructive
  live-process actions; machine-stable output remains Phase 4.

**Gate:** restart an isolated daemon, verify topology/IDs survive while no old
process is claimed live, explicitly restore selected leaves with new
incarnations, and prove a malformed/truncated store cannot erase the last valid
snapshot or execute commands.

### Slice 4 — multiple Dojo windows (complete 2026-07-20)

- Add new/list/select/rename/close window workflows and persist independent
  trees per window.
- Allow `splinterm window --dojo-id ... --window-id ...` and launch one Wayland
  toplevel for the selected resource. Opening or closing it never creates,
  kills, or focuses another daemon window implicitly.
- Track window-local default focus hints only as convenience metadata; each
  connected client keeps its actual focus locally.

**Graphical smoke gate:** after non-graphical tests pass, use the guarded
workspace-8/DP-2 launcher for one case: two toplevels attached to two windows in
one Dojo, independent input/resize, close/reopen continuity, no focus or
placement violation, and full cleanup. Run a broader matrix only after this
single case succeeds.

**2026-07-20 implementation evidence:** the full non-graphical validation
contract and the single guarded graphical case pass. Protocol v16 selects an
exact Dojo/window pair, persists validated window-local default-focus hints, and
keeps actual focus client-local. `tools/run-phase3-slice4-window-smoke.py` mapped
two selected toplevels in one Dojo, verified independent input, observed distinct
PTY resize results (`45x11` and `67x16`), closed and reopened one UI without
altering the other daemon window, and completed cleanup. Active workspace,
active window, and pointer remained unchanged; workspace 8 and the isolated
socket were empty afterward. No broader graphical matrix ran. Evidence:
[`../spikes/artifacts/phase3-slice4-windows/summary.json`](../spikes/artifacts/phase3-slice4-windows/summary.json).

### Slice 5 — pane-view extraction, split rendering, and focus navigation

- Extract per-pane client state from Wayland `App`: snapshot, subscription,
  viewport, history cache, selection, search state, dirty rows, controller
  status, and last dimensions. Keep seat, clipboard offers, IME, SHM pool, and
  Wayland objects window-owned.
- Compute deterministic rectangles from the daemon binary tree using fixed-unit
  ratios, renderer cell constraints, separator widths, and stable rounding.
- Render clipped pane frames into one toplevel backing buffer; damage only panes
  affected by terminal updates, focus chrome, ratio edits, or topology changes.
- Add create split, close, ratio adjustment, and directional/next/previous focus
  bindings. Input, mouse, IME cursor rectangle, selection, local scrollback, and
  resize route only to the focused pane.
- Acquire control lazily for the focused pane and release it on detach or an
  explicit release command; focus change alone must not steal another client's
  lease. Uncontrolled panes remain live observers with visible status.

**Gate:** pure geometry/focus tests precede one guarded graphical case covering
horizontal plus nested vertical split, focus traversal, independent shell input,
resize/reflow, selection, scrollback, IME placement, close/collapse, and reattach.

**Status:** Complete. Pure tests cover stable nested geometry, focus traversal,
clipped neighbor isolation, pane-local resize, inactive detached scrollback, input
encoding, selection, and pane-offset IME rectangles. Because graphical isolation
forbids focusing a test window, interaction semantics are exercised headlessly;
the guarded workspace-8/DP-2 evidence covers static nested panes plus live
single-to-split reconciliation, ratio changes, observer input/control isolation,
exited-pane retention, close/collapse, detach, and reattach without focus or
pointer movement. Evidence is recorded under
`docs/spikes/artifacts/phase3-slice5-panes/`,
`docs/spikes/artifacts/phase3-slice5-dynamic/`, and
`docs/spikes/artifacts/phase3-slice5-final/`. A read-only review found lazy-control,
stale-topology, IME-origin, and inactive-scrollback defects; all received targeted
fixes and regression coverage before final workspace validation. Visible
box-drawing pane chrome is specified separately in
[Plan 0005](0005-pane-divider-styles.md) as a bounded Slice 5 follow-up.

### Slice 6 — simultaneous clients and explicit control transfer

- Expose bounded per-Splint control status to trusted first-party UI without
  leaking unnecessary peer details to untrusted clients.
- Add an explicit takeover handshake: requester asks, current trusted UI can
  release/deny, and forced takeover requires a separate trusted confirmation.
  Timeout defaults to denial; disconnect releases normally.
- Broadcast control-granted/released/revoked/incarnation-changed events so every
  attached first-party client updates non-spoofable chrome immediately.
- Test two clients observing one Splint, clients controlling different Splints,
  denied takeover, accepted transfer, requester/current-owner disconnect at each
  step, grant revocation, and stale messages.

**Gate:** no automatic typing or resize occurs after attach, focus, reconnect,
or another client's topology edit; only the visible lease owner can mutate the
PTY and terminal size.

### Slice 7 — bounded scrollback search

- Add a terminal-actor search command over normal-history rows and optionally the
  visible normal grid. Search literal UTF-8 with explicit case-sensitive or
  Unicode-case-insensitive mode, maximum query bytes, maximum results, deadline,
  and cancellation checkpoints.
- Return stable row IDs, bounded match column ranges, and a short bounded preview;
  never return or log the entire history. Bind pages/cursors to Splint,
  incarnation, terminal revision, and history generation and force restart when
  trim, reflow, clear, resize, or output invalidates the cursor.
- Add a client-local search overlay, next/previous navigation, match highlight,
  and lazy history-page fetch to reveal selected results. Terminal output cannot
  spoof the overlay.
- Cover combining/wide cells, soft wraps, invalid UTF-8 query bounds, duplicate
  text on different row IDs, alternate screen, trim/reflow, huge configured
  history, cancellation, authorization, and concurrent output.

**Performance gate:** search memory remains bounded independently of configured
history capacity and does not measurably stall PTY consumption or another
Splint actor.

### Slice 8 — closure, documentation, and package evidence

- Run the complete headless lifecycle and guarded graphical scenarios, record
  bounded CPU/PSS/queue/search/topology evidence, and verify no workspace-8 or
  process/socket residue.
- Update architecture, roadmap, README, configuration/bindings, packaging and
  upgrade/recovery language. Clearly distinguish detach persistence, daemon
  metadata restore, and explicit process relaunch.
- Record deferred work: persistent scrollback bodies, public automation schema,
  remote/headless access workflow, wire-memory optimization, Nix, and public
  distribution.

## Validation contract for every implementation slice

Run the smallest relevant unit/package tests after each dependency-ordered
change, then before closing a slice run:

```bash
cargo fmt --all --check
cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace
cargo test -p splinterd --test end_to_end -- --test-threads=1
```

Graphical tests run only after those commands pass and only through the guarded
workspace-8/DP-2 procedure in `AGENTS.md`: verify workspace 8 is assigned to
DP-2, inactive, and empty; use pre-map/no-focus rules; abort and clean up on any
placement or focus violation; run one smoke case before any matrix.

## Phase 3 definition of done

Phase 3 is complete when a user can create a Dojo containing multiple windows
and nested Splints; navigate and edit one tree; open, close, and reopen clients
without ending shells; rename, kill, close, and explicitly relaunch resources;
restart the daemon and safely restore metadata without false continuity claims;
search bounded scrollback; and use simultaneous clients under visible,
per-Splint, explicit control semantics. All operations preserve authorization,
backpressure, stale-identity rejection, Foot-derived terminal behavior, private
package usability, and the measured Phase 2 memory/performance baseline.

## Stop gates

Stop and request a new architecture decision if implementation requires:

- more than one writer for topology or persistent state;
- holding a shared daemon lock across actor/filesystem/consent operations;
- implicit control stealing or shared focus;
- executing persisted commands automatically on startup;
- persisting terminal or clipboard bodies without a separate privacy/storage ADR;
- changing the canonical Foot checkout, oracle images, or broad tolerances;
- a graphical slice before its headless identity/lifecycle contracts pass; or
- Phase 4 public automation/remote transport to make Phase 3 work.
