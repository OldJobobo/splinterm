# Plan 0001: Foot terminal kernel and one-Splint vertical slice

- **Status:** Phase 2 complete — Phase 3 next
- **Foundation:** [ADR 0001](../adr/0001-foot-rust-port.md)
- **Reference source:** Foot 1.27.0, commit
  `3c5b584b0eafa772eb4376fb6eaf6643399e190e`

## Execution progress

- [x] Pin and build Foot 1.27.0 at the accepted reference commit.
- [x] Record a reproducible minimal Foot build command and provenance manifest.
- [x] Define the semantic oracle adapter and normalized fixture format.
- [x] Add five source-reviewed fixtures: printable text, soft wrapping, cursor
  positioning, erase-line, and basic SGR.
- [x] Validate fixture structure in CI.
- [x] Implement the test-only Foot semantic state-dump adapter.
- [x] Promote the initial fixtures from `source_reviewed` to `oracle_verified`.
- [x] Add `splinterm-terminal` foundational color, attribute, cell, coordinate,
  cursor, scroll-region, and row representations.
- [x] Record the Phase 1 memory-size baseline and focused representation tests.
- [x] Review and commit the Phase 1 foundational representations.
- [x] Port circular row indexing and lazy row allocation.
- [x] Port screen/scrollback coordinate conversion and viewport-relative row
  tracking.
- [x] Port row erase/fill operations and dirty-state transitions.
- [x] Port full and partial forward/reverse scrolling.
- [x] Port resize without reflow, including cursor clamping and wide-cell cuts.
- [x] Port logical-line reflow with linebreak, wide-cell, composed-width,
  cursor, saved-cursor, and viewport tracking.
- [x] Add deterministic randomized grid-operation invariant coverage.

## Goal

Produce the smallest trustworthy piece of the Foot-to-Rust port:

1. a renderer-independent Rust terminal kernel derived from Foot; and
2. one daemon-owned shell Splint that survives client disconnection.

The result is not yet a graphical terminal. It is a headless walking skeleton
that proves the most important ownership and compatibility boundaries before
Wayland, fonts, rendering, or full multiplexing are added.

## Definition of done

The milestone is complete when all of the following are true:

- `splinterm-terminal` accepts PTY bytes and updates a terminal grid.
- Its implemented behavior is compared against the pinned Foot reference.
- `splinterd` can start one shell in a PTY and feed its output into the kernel.
- Structured input reaches the shell.
- Resize updates both the PTY and terminal grid.
- A client can detach without terminating or blocking the shell.
- Reattachment returns a current snapshot and subsequent ordered updates.
- Slow or disconnected clients cannot backpressure PTY consumption.
- Malformed and arbitrarily chunked terminal input cannot panic the kernel.
- Sensitive terminal access is not exposed through an unauthenticated public
  API.

## Non-goals

This milestone deliberately excludes:

- Wayland windows and compositor integration;
- font discovery, shaping, glyph caches, and rendering;
- multiple Splints or layout mutation;
- durable process continuity across daemon or host restart;
- persistent on-disk scrollback;
- complete Foot configuration compatibility;
- sixel and other image protocols;
- complete OSC/DCS feature parity;
- clipboard, selection, search, URLs, IME, and mouse input;
- MCP, editor plugins, and supported third-party automation;
- NixOS and tertiary distribution packaging.

The design must leave room for these features without pretending they are part
of the first milestone.

## Architectural boundaries

### `splinterm-terminal` — new crate

A direct Rust port of Foot's terminal model and parser behavior.

It must not depend on:

- Tokio or another async runtime;
- PTYs or child processes;
- Wayland or rendering libraries;
- `splinterm-protocol` or a wire encoding;
- persistence or filesystem policy.

It may expose plain Rust snapshots and events. Protocol conversion belongs in
higher-level crates.

### `splinterm-pty` — new crate

Owns Unix PTY and child-process mechanics:

- allocate master/slave;
- spawn a command with a controlling terminal;
- read and write bytes;
- resize with rows and columns;
- observe child exit;
- close and signal intentionally.

The first implementation choice requires a focused spike. Candidates are a
small audited Rust/Linux layer, a temporary Foot C bridge, or `portable-pty` if
it reproduces required Foot behavior. Whichever path is selected must be hidden
behind a Splinterm-owned interface.

### `splinterd`

Owns the live Splint:

- PTY master and child;
- terminal kernel instance;
- input ordering;
- resize ownership;
- snapshot and update revisions;
- bounded client subscriptions;
- detach/reattach lifetime.

### `splinterm-protocol`

Eventually carries handshake, authorization, requests, snapshots, updates, and
resynchronization. The terminal crate's internal structs must not become the
wire format.

### `splinterm`

Acts as a development control client during this milestone. It does not render
a terminal window yet.

## Foot source map

Port behavior from the pinned local source rather than reconstructing it from
another terminal engine.

| Rust area | Primary Foot source |
| --- | --- |
| colors and attributes | `terminal.h` |
| cells and coordinates | `terminal.h` |
| rows and row metadata | `terminal.h`, `grid.c` |
| circular grid and scrollback | `terminal.h`, `grid.h`, `grid.c` |
| cursor and scroll regions | `terminal.h`, `terminal.c` |
| byte-state machine | `terminal.h`, `vt.h`, `vt.c` |
| terminal commands | `commands.c`, `terminal.c` |
| CSI handling | `csi.c` |
| OSC handling | `osc.c` |
| DCS handling | `dcs.c` |
| UTF-8/character helpers | `char32.c`, `composed.c`, related headers |
| PTY allocation and spawn | `terminal.c`, `slave.c`, `spawn.c` |
| child exit | `reaper.c` |
| resize/reflow | `grid.c`, `terminal.c` |

Every translated module should record the Foot source file and pinned revision
in its module documentation or provenance manifest.

## Proposed terminal-kernel layout

```text
crates/splinterm-terminal/
├── Cargo.toml
├── src/
│   ├── lib.rs
│   ├── cell.rs          # attributes, colors, cell representation
│   ├── coord.rs         # coordinates, ranges, scroll regions
│   ├── row.rs           # row storage and metadata
│   ├── grid.rs          # screen, scrollback, resize, reflow
│   ├── cursor.rs        # cursor state and saved cursor
│   ├── terminal.rs      # terminal coordinator and modes
│   ├── event.rs         # damage, PTY reply, title, bell, etc.
│   └── vt/
│       ├── mod.rs       # streaming Foot-derived state machine
│       ├── command.rs   # C0/C1 and core terminal commands
│       ├── csi.rs
│       ├── osc.rs
│       └── dcs.rs
└── tests/
    ├── fixtures/
    ├── parser.rs
    ├── grid.rs
    ├── chunking.rs
    └── foot_parity.rs
```

This layout is provisional. It may change when the first Foot structures expose
better Rust ownership boundaries.

## Initial public API sketch

The API should remain small and independent of daemon policy:

```rust
pub struct Terminal {
    // Private Foot-derived terminal state.
}

impl Terminal {
    pub fn new(columns: u16, rows: u16, config: TerminalConfig) -> Self;
    pub fn advance(&mut self, bytes: &[u8]);
    pub fn resize(&mut self, columns: u16, rows: u16);
    pub fn snapshot(&self) -> TerminalSnapshot<'_>;
    pub fn drain_events(&mut self) -> impl Iterator<Item = TerminalEvent> + '_;
}
```

Likely event categories:

- damaged rows or ranges;
- bytes to write back to the PTY;
- title/app-id changes;
- bell;
- mode changes relevant to the client;
- full-resnapshot requirement.

This is a planning sketch, not a frozen API. Avoid exposing concrete collection
choices or serialized DTOs prematurely.

## Work phases

### Phase 0: establish the Foot oracle

Before translating terminal behavior:

1. Confirm the Foot checkout is clean and pin its commit in test metadata.
2. Build Foot with a documented reproducible command.
3. Decide how semantic state will be extracted for comparison.
4. Store input transcripts and expected state independently of Rust code.
5. Record intentional unsupported behavior per fixture.

The semantic oracle is a required spike because Foot does not expose a ready
machine-readable dump of every cell attribute and mode. Evaluate, in order:

1. a test-only state-dump adapter built against the pinned Foot source;
2. a maintained patch applied to a disposable Foot build tree;
3. targeted C harnesses for grid/parser algorithms;
4. text/screenshot comparison only where semantic extraction is impractical.

Do not modify the canonical `~/Playground/foot` checkout during normal tests.
Use a build copy, patch application, or dedicated worktree.

**Deliverables**

- documented Foot build command;
- `tools/foot-oracle/` design or implementation;
- first fixtures for printable text, wrapping, cursor movement, erase, and SGR;
- provenance manifest containing the Foot commit.

### Phase 1: port foundational types

Port the smallest state types from `terminal.h`:

- color source and color value representation;
- cell attributes;
- cell content and spacer/continuation semantics;
- coordinates, ranges, cursor, and scroll regions;
- row storage and dirty/linebreak metadata.

Rust layout does not need to reproduce C ABI layout, but memory size is a
performance requirement. Add size measurements and benchmarks rather than
unsafe packed representations.

**Tests**

- defaults and attribute transitions;
- equality/snapshot behavior;
- wide-cell continuation invariants;
- row initialization and reset;
- memory-size regression reporting.

### Phase 2: port grid behavior

Port Foot's circular grid and screen operations:

- row allocation and lookup;
- screen versus scrollback coordinates;
- full and partial scrolling;
- erase/fill operations;
- dirty-row tracking;
- resize without reflow;
- resize with reflow after the basic grid is stable.

Preserve Foot's wrap and linebreak semantics. Do not flatten history to strings.

The Phase 2 kernel scope excludes URI/extended-underline ranges,
shell-integration markers, selections, and sixels. Those states belong to
features explicitly deferred by this milestone and must be translated alongside
their owning features. Cell content, attributes, hard/soft linebreaks, cursors,
viewport position, and scrollback ordering are preserved here.

**Tests**

- circular index wraparound;
- scroll-region boundaries;
- resize growth and shrinkage;
- reflow tracking points;
- wide and combining characters across resize;
- randomized operation sequences with invariant checks.

### Phase 3: port the VT state machine

Port Foot's streaming parser from `vt.c` and its state in `terminal.h`.
Recognition and dispatch should remain chunk-independent: feeding one byte at a
time or one complete buffer must result in identical state.

Bring up handlers in this order:

1. printable ASCII and UTF-8;
2. C0 controls: CR, LF, BS, HT, BEL;
3. wrapping and scrolling;
4. basic CSI cursor movement;
5. erase line/display;
6. SGR reset, styles, 16/256/RGB colors;
7. save/restore cursor and scroll regions;
8. basic terminal queries and PTY replies;
9. alternate screen and core DEC modes;
10. bounded OSC title and palette operations.

The parser itself should recognize Foot's complete state transitions early even
when some semantic handlers intentionally report `Unsupported` during this
milestone. Unsupported sequences must terminate safely and preserve parser
synchronization.

**Tests**

- Foot differential fixtures for every implemented operation;
- every possible input split point;
- malformed UTF-8 and incomplete escape sequences;
- parameter overflow and excessive parameter counts;
- OSC/DCS terminator and length limits;
- fuzz target asserting no panic and stable invariants.

### Phase 4: define snapshots and events

Create project-owned read models for:

- dimensions and revision;
- visible rows and cells;
- cursor and modes;
- bounded scrollback segment;
- damage and non-grid events.

Snapshots must represent semantic terminal state, not renderer artifacts.
Glyphs, shaped runs, textures, and Wayland buffers never belong here.

Add a monotonically increasing terminal revision. A future client that misses
updates must request a new snapshot rather than guessing.

### Phase 5: PTY/process spike

Define a `splinterm-pty` interface before selecting an implementation:

```rust
pub trait PtySession {
    fn resize(&mut self, size: PtySize) -> Result<()>;
    fn write(&mut self, bytes: &[u8]) -> Result<usize>;
    fn try_wait(&mut self) -> Result<Option<ExitStatus>>;
}
```

The actual read integration may use an owned file descriptor rather than a
trait method so Tokio can register it safely.

Validate:

- controlling terminal and process-group behavior;
- shell selection and argv boundaries;
- current working directory;
- environment construction;
- `TERM` value;
- resize signaling;
- child exit and signal behavior;
- no unsafe allocator/runtime work in a post-fork child;
- descriptor inheritance and close-on-exec behavior.

Record the chosen implementation in a follow-up ADR before it becomes a public
crate contract.

### Phase 6: one live Splint in `splinterd`

Add a daemon runtime object separate from serializable `splinterm-core` data:

```text
LiveSplint
├── SplintId
├── process incarnation ID
├── PTY session
├── terminal kernel
├── input queue
├── terminal revision
└── subscriber set
```

Use one logical task/actor to serialize PTY output, input, resize, snapshots,
and exit. The actor must continue consuming the PTY when there are no clients.

Start with one Splint inside one Dojo/window. Do not implement tree mutation yet.

### Phase 7: secure development attach path

Before exposing terminal content through IPC, add the minimum protocol
foundation already required by the research plan:

- bounded frames;
- request IDs;
- version-range handshake;
- stable errors;
- peer-UID verification;
- per-operation authorization;
- maximum outstanding requests;
- bounded subscription queues;
- revision gaps and `ResyncRequired`;
- explicit detach and cancellation.

Initial terminal access may remain test-only behind an explicit development
flag until the trusted graphical consent UI exists. Do not accidentally declare
an unrestricted same-UID socket to be the supported AI API.

Development operations:

- create one shell Splint;
- send literal input bytes or structured text;
- resize;
- request visible-grid snapshot;
- subscribe to terminal updates;
- detach;
- reattach;
- terminate explicitly.

### Phase 8: end-to-end validation

Automate a headless scenario:

1. start `splinterd` with an isolated runtime directory;
2. create a shell Splint;
3. run `printf`, `pwd`, color, cursor, and resize fixtures;
4. verify semantic snapshots;
5. detach the client;
6. run additional output while detached;
7. reattach and verify the process and grid remained current;
8. stall a subscriber until its queue overflows;
9. verify the daemon continues reading and forces that client to resynchronize;
10. terminate the Splint and verify child cleanup and exit reporting.

## Test strategy

### Unit tests

Place algorithm tests beside the ported modules. Keep them small and traceable
to Foot behavior.

### Differential fixtures

Each fixture should contain:

- raw input bytes;
- initial dimensions and relevant config;
- expected cells/attributes;
- expected cursor, modes, title, and replies;
- Foot revision;
- whether Splinterm intentionally diverges.

### Property tests

At minimum:

- grid indices always remain valid;
- cursor is valid after completed operations;
- row widths match configured columns;
- wide-cell leaders and continuations remain consistent;
- resize/reflow preserves logical content according to Foot semantics;
- arbitrary input chunking does not change final state.

### Fuzzing

Add fuzz targets after the parser and grid APIs stabilize:

- arbitrary parser input;
- parser input plus random chunk boundaries;
- random resize and terminal-operation sequences;
- snapshot generation after arbitrary valid state transitions.

### Benchmarks

Track from the beginning:

- parser throughput;
- allocations per input MiB;
- memory per cell and per scrollback line;
- full-screen scroll cost;
- resize/reflow latency;
- snapshot creation cost.

Foot is the comparison baseline. Optimize only after correctness differences are
understood.

## Dependency policy

- Do not add another terminal engine as a dependency.
- Prefer standard library and small infrastructure crates.
- Keep terminal semantics in project-owned Rust code derived from Foot.
- Audit licenses before adding PTY, Unicode, font, or platform crates.
- Do not expose dependency types in public Splinterm APIs.
- Keep `unsafe` forbidden in normal project code; isolate and review any future
  exception through a dedicated ADR and narrowly scoped crate/module.

## Implementation commits

Keep review units narrow. A suggested sequence is:

1. `Add Foot oracle fixtures and provenance manifest`
2. `Add splinterm-terminal foundational types`
3. `Port Foot grid and scrolling behavior`
4. `Port Foot VT state machine and basic controls`
5. `Port basic CSI and SGR behavior`
6. `Add terminal snapshots, events, and revisions`
7. `Add splinterm-pty abstraction and selected backend`
8. `Run one daemon-owned shell Splint`
9. `Add bounded attach and resynchronization protocol`
10. `Add detachable one-Splint integration tests`

Each commit must compile and carry tests for the behavior it introduces.

## Risks and mitigations

### Hidden coupling in Foot

Foot's parser and handlers mutate a large shared terminal structure.

**Mitigation:** preserve behavior while introducing Rust module boundaries;
avoid redesigning semantics and ownership simultaneously inside one commit.

### False parity from text-only comparison

Two terminals may print the same text while differing in attributes, modes,
wrap flags, or replies.

**Mitigation:** build semantic state extraction before claiming parity.

### Memory regression

Naive Rust enums and owned metadata can make each cell substantially larger than
Foot's compact representation.

**Mitigation:** track cell/row sizes and realistic scrollback RSS from Phase 1;
optimize with safe representations before large histories are enabled.

### Parser denial of service

Unbounded parameters and OSC/DCS payloads can consume memory or CPU.

**Mitigation:** preserve Foot's caps where present and introduce documented hard
limits before accepting untrusted PTY output.

### PTY fork safety

Forking after a multithreaded runtime starts can invoke unsafe child-side code.

**Mitigation:** prefer APIs that reach `exec` through an audited path, isolate
the operation, and test descriptor/process-group behavior directly.

### Premature protocol coupling

Serializing terminal internals would make every engine change a wire break.

**Mitigation:** define separate snapshots/events and explicit conversion in the
protocol layer.

## Review gates

Do not proceed to the Wayland/rendering milestone until:

- foundational Foot parser/grid fixtures pass;
- the oracle method is documented and repeatable;
- one shell Splint survives detach/reattach;
- stalled-client behavior is bounded and tested;
- memory and parser benchmarks have a recorded baseline;
- PTY ownership and shutdown behavior pass integration tests;
- unsupported behavior is listed rather than silently treated as parity.

## Immediate next task

Phase 0 is complete for the initial corpus. Begin Phase 1:

> Create `splinterm-terminal` and port Foot's color source, attributes, cell,
> coordinate, range, cursor, scroll-region, and row types with provenance and
> focused tests.

Do not begin grid algorithms until the foundational representations and their
memory-size baseline are reviewed.
