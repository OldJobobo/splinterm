# Plan 0034: Alpha3 saved Lair layouts

- **Status:** Complete for `0.1.0-alpha3`; implementation, review, and installed-package graphical acceptance recorded
- **Date:** 2026-08-12
- **Product authority:** A saved Lair is an explicit durable workspace recipe,
  not a process checkpoint or automatic command-execution mechanism
- **Depends on:** durable topology and explicit restore from Plan 0018, preset
  launch metadata from Plan 0027, and the Lair lifecycle decisions in this plan

## Decision

Make the daemon's existing durable topology and launch metadata understandable
and controllable as a first-class **Save Lair Layout** workflow before publishing
`0.1.0-alpha3`.

A saved Lair layout retains:

- the Lair name and durable identity;
- ordered Dojos and their names;
- every Dojo's binary Splint tree;
- split axes and proportional split ratios;
- the default focused Splint for each Dojo;
- Splint names;
- each Splint's known launch working directory;
- an explicit launch recipe when Splinterm owns one;
- shell/login-shell and bounded scrollback launch policy; and
- last-known rows and columns as bounded launch hints.

Restoration remains deliberate. Loading durable metadata may reconstruct the
workspace recipe, but it must never start saved applications or shells until the
user invokes an explicit restore action and confirms the stated scope.

## Existing foundation

The preliminary implementation already provides most of the storage model:

- a persistent Lair owns one or more durable Dojos;
- each Dojo owns a binary `LayoutNode` tree with bounded `SplitRatio` values;
- each Splint stores a title, launch cwd, command argv, shell policy, scrollback
  limit, last rows/columns, and explicit restore policy;
- persistence records only persistent Lairs and converts live leaves to exited,
  restorable metadata;
- `restore`, `restore-dojo`, and `restore-lair` explicitly relaunch saved recipes;
  and
- durable validation bounds document size, topology depth, collection sizes,
  command size, paths, names, geometry, and duplicate identities.

The missing work is user-visible state, save semantics, honest application
classification, preview/confirmation, picker presentation, lifecycle policy,
and end-to-end acceptance.

## Application restoration boundary

“Save the applications in the layout” means saving **known launch recipes**, not
capturing arbitrary process state.

### Restorable application

A Splint may identify a restorable application when Splinterm created it from an
explicit structured launch, for example:

- a preset leaf with direct argv;
- a command-bearing XDG launch whose lifecycle permits persistence; or
- another bounded application launch represented as exact argv plus a validated
  working directory and launch policy.

The saved representation must preserve argument boundaries. It must never
serialize a shell command string merely to replay it later.

### Shell or unknown foreground application

When a Splint was launched as a shell, Splinterm must describe its recipe as a
shell even if an editor, multiplexer, REPL, or other foreground descendant is
currently running inside it. The daemon must not infer a restorable application
from `/proc`, terminal titles, shell prompts, terminal output, or process names.

Saving does not preserve:

- process memory or kernel PTYs;
- current editor buffers or application-internal state;
- shell history, variables, functions, aliases, jobs, or current prompt state;
- the shell's current directory unless an accepted, authenticated integration
  has explicitly updated durable launch metadata;
- environment values, secrets, terminal bodies, scrollback bodies, clipboard
  contents, or image bodies; or
- arbitrary descendants started interactively inside a shell.

The save preview must label such leaves clearly as **Shell** or **No restorable
application recipe** rather than guessing.

## Size and layout contract

Split ratios are the authoritative saved size representation. Restoring into a
Window with different dimensions must preserve the tree and proportional ratios,
then derive valid cell rectangles for the new content area.

Last-known rows and columns are bounded launch hints, not a promise of identical
pixel dimensions. After attachment, ordinary authoritative resize negotiation
updates each PTY to the actual restored layout. Minimum-pane constraints must
fail clearly rather than silently changing the saved tree or dropping a Splint.

## Alpha3 scope

### 1. Define Lair lifecycle states

Represent and document at least:

- **Live** — one or more Splints are running;
- **Detached** — live daemon-owned work exists without a graphical attachment;
- **Saved** — the Lair is explicitly retained as a durable workspace recipe;
- **Restorable** — no Splints are live, but validated launch/layout metadata
  remains;
- **Pinned** — protected from automatic retirement; and
- **Disposable** — eligible for bounded retirement after all Splints exit.

Preset-derived provenance may be shown when known, but a saved Lair must not
depend on the original preset file remaining unchanged or present.

State transitions must be revision-bound and atomic. Saving or pinning an active
Lair must not restart, resize, detach, or otherwise disturb its processes.

### 2. Add explicit save and restore workflows

Provide trusted human actions for:

- Save Current Lair Layout;
- Pin/Unpin Current Lair;
- Preview Saved Lair Layout;
- Restore Saved Lair;
- Restore one selected Dojo; and
- Delete or retire saved metadata with destructive confirmation.

The command palette and Lair/Dojo picker should expose these actions where the
captured target and current state make them valid. Unavailable actions remain
visible or omitted according to the existing trusted-surface policy, but must
never retarget another Lair after asynchronous topology changes.

A save preview must summarize, without terminal bodies:

- Lair and Dojo names;
- Splint count and tree shape;
- proportional split sizes;
- each leaf's name and launch cwd;
- **Application: argv executable**, **Shell**, or **No restorable recipe**;
- which leaves will relaunch; and
- which metadata will not be restored.

### 3. Preserve exact durable layout data

Prove save/load round trips preserve:

- Lair, Dojo, and Splint identities for ordinary restoration;
- Dojo order, names, and default focus;
- every branch axis and ratio;
- leaf order and Splint names;
- exact structured argv and validated launch cwd;
- shell/login-shell and scrollback policy;
- bounded last-known rows and columns; and
- saved, pinned, disposable, and provenance state after schema migration.

Saving must use the existing clone-mutate-persist-install-publish transaction
ordering. Persistence failure must leave the live topology and previous durable
generation unchanged.

### 4. Make restoration safe and legible

Before execution, restoration must:

- operate on an exact captured Lair/Dojo revision and identity;
- validate every launch recipe, directory, bound, and topology invariant again;
- show the number of Splints and applications/shells that will start;
- require explicit confirmation when restoring more than one process;
- reject missing or changed directories rather than substituting another cwd;
- prevent partial topology publication if preparation fails;
- report per-leaf launch failure without discarding the saved recipe or other
  successfully restored leaves; and
- never set automatic relaunch merely because a Lair was saved or pinned.

Existing automation restore operations retain their separate policy and scope
requirements. Adding a human save surface must not broaden machine authority.

### 5. Reconcile retention behavior

- disposable, fully exited low-value Lairs may remain eligible for the existing
  bounded retirement policy;
- Saved and Pinned Lairs must never be selected for automatic retirement;
- active or detached-live Lairs must never be retired automatically;
- capacity handling must fail or select only a documented eligible candidate;
  and
- deletion of a Saved or Pinned Lair requires explicit destructive confirmation
  and must not silently terminate live Splints.

## Explicitly outside alpha3

- checkpointing or migrating live processes, PTYs, memory, or application state;
- inspecting arbitrary foreground descendants to guess what should relaunch;
- replaying shell command strings, shell history, terminal output, or environment;
- restoring editor buffers, browser tabs, REPL state, or unsaved files;
- automatic application launch on daemon startup, login, package upgrade, or
  picker display;
- reusable cross-Lair templates or cloning one saved Lair into multiple new
  identities;
- synchronizing saved layouts between machines;
- live current-directory capture without a separately designed authenticated
  shell-integration contract; and
- changing public automation schemas beyond a separately reviewed versioned
  extension.

## Validation milestones

### Milestone 1 — lifecycle and schema

- define the exact saved/pinned/disposable state model and migration;
- add bounded schema and round-trip tests for layout, ratios, launch recipes,
  geometry hints, and provenance;
- prove legacy durable topology migrates without becoming implicitly Saved or
  automatically executable;
- pass core and daemon persistence tests, formatting, strict Clippy, and
  `git diff --check`.

### Milestone 2 — trusted save/preview UI

- implement exact-target save, pin, preview, restore, and delete actions;
- add body-free preview models and renderer tests;
- prove modal isolation, cancellation defaults, stale-target rejection, and
  persistence-failure rollback;
- update palette, picker, usage, lifecycle, recovery, and privacy documentation.

### Milestone 3 — restoration and retention matrix

- test one- and multi-Dojo Lairs with nested horizontal/vertical ratios;
- test explicit applications, shells, mixed recipes, missing cwd, invalid argv,
  partial spawn failure, capacity, migration, and concurrent revision changes;
- prove Saved/Pinned protection and Disposable-only retirement;
- run one serial workspace validation on the coherent alpha3 release state and
  obtain fresh read-only lifecycle/security review.

### Milestone 4 — packaged graphical acceptance

After separate approval under the repository graphical-testing rules, use the
installed adjacent trusted client and daemon in one guarded sequence to:

1. create a Lair with multiple Dojos, nested unequal splits, one explicit safe
   application recipe, and one ordinary shell;
2. save and preview the Lair without exposing terminal bodies;
3. verify previewed tree, ratio, cwd, and application/shell classifications;
4. end the test processes, explicitly restore the saved Lair, and verify tree,
   proportional pane sizes, default focus, and expected launch behavior;
5. prove no application starts before confirmation;
6. prove pinned saved state is protected from the bounded retirement path; and
7. remove all test topology and restore Window, focus, workspace, monitor,
   geometry, configuration, and package state.

Abort on wrong-window input, unexpected process launch, unrelated topology
mutation, ratio drift outside documented integer projection, or incomplete
cleanup.

## Installed-package graphical evidence (2026-08-13)

- A multi-Dojo Lair with nested splits and mixed shell/application recipes was
  saved and previewed without terminal bodies. The durable tree, split ratios,
  stable IDs, cwd/launch classifications, and default focus survived process
  exit and explicit restoration.
- Protected Saved/Pinned recipes retained exited leaves. Restore targeted only
  `Exited` leaves; mixed live/exited prompts reported the exact restorable count,
  stayed within protocol dimensions, defaulted to Cancel, and executed nothing
  before explicit confirmation.
- Termination separately targeted `Starting`/`Running` leaves. The final trusted
  human UI confirmation atomically removed the exact four-process Lair; remote,
  mismatched-local, and automation identities remain outside that bypass.
- Queue saturation during nested resize now defers and retries instead of
  advancing the applied size, while disconnect remains fatal. The final package
  completed restore and destructive cleanup without crashes or authorization
  errors.

## Alpha3 acceptance

Plan 0034 is complete only when:

- Save Lair Layout has an explicit, documented lifecycle meaning;
- exact tree shape, split ratios, known launch recipes, launch cwd, and bounded
  geometry hints survive durable round trips;
- shells and unknown foreground applications are described honestly and never
  guessed;
- no saved command executes automatically;
- Saved/Pinned Lairs are protected from automatic retirement;
- focused and serial validation evidence is recorded;
- a fresh read-only review has no unresolved blockers; and
- separately approved packaged graphical acceptance is recorded.

This plan does not authorize installation, graphical testing, pushing, candidate
dispatch, promotion approval, AUR publication, or release publication.
