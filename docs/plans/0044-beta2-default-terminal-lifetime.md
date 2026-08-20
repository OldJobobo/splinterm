# Plan 0044: Beta 2 default terminal lifetime

- **Status:** Implementation accepted after complete non-graphical, clean-package, isolated graphical, and post-fix review gates; unpublished
- **Date:** 2026-08-19
- **Target release:** `v0.1.0-beta2`
- **Release line:** `maint/0.1`, followed by a separately reviewed forward-port to `main`
- **Product authority:** Ordinary unnamed local graphical terminals may be client-bound by user configuration; explicit naming or tab organization expresses durable intent
- **Includes:** the reviewed startup font-family hotfix at `27321e0a388775fb545662835afe69c9557176dc`
- **Depends on:** the published `v0.1.0-beta1` release, client-owned transient Lairs, Dojo tabs, topology revisions, and protected release automation

## Decision

Publish an immutable Beta 2 release containing two bounded changes:

1. the reviewed startup font-family hotfix, so newly launched clients derive
   regular, bold, italic, and bold-italic from the selected Fontconfig family;
   and
2. a backward-compatible user choice for whether ordinary unnamed local
   graphical terminals start persistent or client-bound.

Add these settings:

```ini
[multiplexer]
persistent-by-default=yes
persist-on-tab-organization=yes
```

Both default to `yes`. Existing configurations therefore retain Beta 1
behavior. `persistent-by-default=no` changes only ordinary unnamed local
human-graphical creation. `persist-on-tab-organization=yes` promotes a
client-bound Lair to persistent when its owning Window creates another Dojo tab
or explicitly names or renames a Dojo tab.

Promotion applies to the complete Lair, including every existing Dojo and
Splint. It is atomic, durable, and irreversible for that Lair. After promotion,
closing the Window detaches instead of terminating the Lair.

## User-visible behavior contract

### Ordinary creation governed by the setting

When all of the following are true, `persistent-by-default` chooses the new
Lair's lifetime:

- the caller is a local trusted human graphical client;
- creation opens a fresh graphical terminal;
- no explicit Lair name was supplied; and
- no explicit command was supplied.

The governed entry points are:

- commandless `splinterm-xdg-terminal-exec`, including the Omarchy desktop icon
  and `SUPER+ENTER` path;
- bare `splinterm launch`;
- **New** in the Recent Dojos picker; and
- the in-Window **New Terminal** action that creates a fresh Lair.

With `persistent-by-default=yes`, these paths retain Beta 1 behavior. With
`persistent-by-default=no`, they create one client-bound transient Lair whose
configured shell starts normally. Closing its owning Window terminates its
processes and removes it from topology unless it was promoted first.

### Creation that remains persistent

The setting does not weaken an explicit durable or non-basic request. These
paths remain persistent:

- `splinterm new NAME`;
- `splinterm launch --name NAME`;
- `splinterm launch -- COMMAND...`;
- preset materialization;
- automation and MCP creation;
- remote graphical or remote automation creation;
- restore and relaunch operations; and
- any request whose public contract already says it creates persistent
  topology.

A generated collision-resistant Lair name and the generated initial `Dojo 1`
label are implementation identities, not explicit user naming, and do not force
persistence.

Command-bearing `splinterm-xdg-terminal-exec -- COMMAND...` retains its existing
client-bound transient contract regardless of the new setting.

### Tab organization and promotion

With `persist-on-tab-organization=yes`, either of these actions against a
transient Lair promotes the Lair before completing the requested tab mutation:

- creating an additional Dojo tab; or
- assigning or changing a Dojo's explicit user-visible name.

The generated initial `Dojo 1` label does not promote. Merely focusing,
reordering, opening, or closing a view does not promote. Renaming a Lair after
creation is not a tab-naming action and does not implicitly change lifetime.

Promotion and the tab mutation form one topology-revision-checked transaction.
If validation, authorization, process admission, naming, topology revision, or
publication fails, neither promotion nor the tab mutation commits.

With `persist-on-tab-organization=no`, a transient Lair may contain multiple
Dojo tabs and explicit tab names while remaining owned by its Window. Closing
that Window still terminates and removes the complete Lair.

### Recency, restore, and visibility

- Unpromoted transient Dojos never enter Recent Dojos and cannot be reopened.
- Promotion may add the promoted Dojo to local recency only after the atomic
  topology mutation succeeds.
- Exited transient topology is removed rather than retained for restore.
- Persistent restore, history, and retention behavior remain unchanged.
- The trusted UI must make transient versus persistent state visible before a
  destructive Window close can be mistaken for detach.

## Safety and ownership invariants

1. Only a local trusted human graphical client may create a commandless
   client-bound Lair or promote one through tab organization.
2. Terminal output remains untrusted and cannot select lifetime, trigger
   promotion, name a tab, or retain a Lair.
3. Automation policy, MCP scopes, remote relay authority, and machine schemas do
   not inherit graphical lifetime authority.
4. Every transient Lair has exactly one live owner lease. Owner disconnect,
   initial-process exit, or explicit termination retires it through the existing
   identity-checked transaction.
5. A Window may own more than one transient Lair, but each Lair keeps a distinct
   owner connection or equivalent independently revocable lease. Do not weaken
   the current one-transient-Lair-per-connection invariant merely to simplify
   tab management.
6. Promotion must prove that the requesting connection owns the current
   transient lease. A different trusted client may not race the owner and retain
   its Lair accidentally.
7. Successful promotion removes the transient lease before owner disconnect can
   reap the Lair. Failed promotion leaves the original lease and lifetime
   unchanged.
8. Lifetime, lease removal, tab mutation, topology revision, runtime admission,
   persistence write, and publication are one ordered transaction.
9. No configuration or CLI spelling may silently reinterpret an explicitly
   named, command-bearing, preset, remote, automation, or MCP request as
   transient.
10. Beta 1 release assets remain immutable. Beta 2 uses a new version, tag,
    manifest, packages, checksums, and publication receipt.

## Non-goals

Beta 2 does not:

- reload font families in already-open Windows;
- make transient Lairs reboot-restorable or retain them after owner loss;
- expose transient creation or promotion to automation, MCP, or remote clients;
- change command-bearing XDG lifetime behavior;
- change persistent history compaction, restore, relaunch, or upgrade handoff;
- add arbitrary lifetime flags to every CLI command;
- infer durable intent from terminal content, shell behavior, current working
  directory, process duration, generated names, or tab focus; or
- publish, install, replace `/usr/bin/splinterm`, update AUR, or run graphical
  tests without the separate approvals required by repository policy.

## Release-line preparation

`origin/maint/0.1` and `v0.1.0-beta1` currently diverge from their Alpha3.3
merge base: the maintenance branch contains the reviewed Beta 1 runtime patches,
while the published tag contains their forward-port and Beta 1 release metadata.
Do not begin implementation on either side of that divergence as if it were a
complete Beta 2 base.

Before Milestone 1:

1. inventory the exact commits unique to `origin/maint/0.1` and
   `v0.1.0-beta1`;
2. create one reviewed coordinator branch from `origin/maint/0.1`;
3. reconcile the published Beta 1 version, package, release, and status metadata
   without replaying duplicate runtime changes;
4. prove the resulting tree represents the published Beta 1 behavior plus the
   accepted maintenance fixes;
5. merge that coordinator through the normal review boundary; and
6. create each Beta 2 implementation branch from the resulting reviewed
   `origin/maint/0.1`.

The coordinator is preparation only. It must not include the font hotfix,
lifetime feature, version bump, candidate construction, or publication.

## Implementation milestones

### Milestone 1 — configuration and pure lifetime selection

Expected files:

- `crates/splinterm/src/config.rs`
- `crates/splinterm/src/app/session_catalog.rs`
- `config/splinterm/config.ini`
- focused config and request-construction tests

Work:

- add complete `AppConfig` booleans for `persistent_by_default` and
  `persist_on_tab_organization`;
- parse only `multiplexer.persistent-by-default` and
  `multiplexer.persist-on-tab-organization` through the existing strict boolean
  parser;
- default both values to `true`;
- define one pure launch classifier from caller authority, graphical ownership,
  explicit name, explicit command, entry point, and configuration to
  `Persistent` or `ClientBound`;
- keep remote, automation, MCP, preset, named, command-bearing, restore, and
  relaunch requests on explicit persistent paths; and
- reject invalid values and report unknown near-miss keys through existing
  configuration diagnostics.

Focused tests must cover the complete decision table, including omission,
accepted true/false spellings, invalid values, generated versus explicit names,
empty versus explicit command vectors, local versus remote semantics, and every
governed entry point.

**Gate:** no daemon or graphical behavior changes in this milestone. The pure
matrix must be reviewable before protocol and lease work begins.

### Milestone 2 — commandless transient creation and per-Lair ownership

Expected files:

- `crates/splinterm-protocol/src/lib.rs`
- `crates/splinterm/src/app/session_catalog.rs`
- `crates/splinterm/src/app/sessions.rs`
- `crates/splinterm/src/app/topology_manager.rs`
- `crates/splinterd/src/main.rs`
- protocol, daemon, and client integration tests

Work:

- permit a trusted local transient request with an empty command vector when its
  launch parameters resolve the configured shell safely;
- retain the current direct-command requirement for command-bearing XDG
  behavior and reject commandless transient requests from remote or automation
  authority;
- route desktop startup, bare launch, picker New, and in-Window New Terminal
  through the shared lifetime classifier;
- give every transient Lair an independent owner connection or equivalent lease
  handle retained by its Window;
- let one Window own multiple transient Lairs without allowing one connection to
  ambiguously own several leases;
- release each owner handle only after that Lair retires, promotes, or the Window
  closes; and
- keep persistent creation on the existing request paths.

Focused tests must prove commandless configured-shell creation, owner-disconnect
retirement, initial-process-exit retirement, multiple independently owned
transient Lairs in one Window, persistent-path non-regression, recency exclusion,
and fail-closed authority checks.

**Gate:** closing an unpromoted Window leaves no process, topology, recency, or
lease residue. No graphical test is required yet.

### Milestone 3 — atomic promotion through tab organization

Expected files:

- private protocol request and response definitions
- `crates/splinterm/src/app/topology_manager.rs`
- tab action/rename routing
- `crates/splinterd/src/main.rs`
- core persistence and topology transaction tests

Work:

- add an owner-only, topology-revision-checked transaction that can promote a
  transient Lair and create or rename a Dojo atomically;
- route transient-Lair tab mutations through that Lair's owner handle;
- apply promotion only when `persist-on-tab-organization=yes`;
- remove the lease and persist the complete Lair before publishing the successful
  mutation;
- keep the Lair transient when the setting is `no`;
- update trusted tab chrome so lifetime is visible without relying on terminal
  content; and
- reconcile local recency only after successful promotion.

Tests must inject failures before validation, persistence, publication, and
lease removal and prove there is no partially promoted or partially renamed
state. Race tests must cover owner disconnect versus promotion, stale topology
revision, duplicate tab names, process exit, and a second client attempting to
promote a lease it does not own.

**Gate:** a promoted Lair survives owner disconnect and remains reopenable; an
unpromoted multi-tab or renamed Lair is fully reaped on owner disconnect.

### Milestone 4 — startup font hotfix backport

Expected source authority:

- commit `27321e0a388775fb545662835afe69c9557176dc`
- `docs/plans/artifacts/0038-font-startup-hotfix/README.md`
- `docs/reviews/2026-08-18-font-family-startup-hotfix.md`

Work:

- backport the reviewed behavior onto the reconciled Beta 2 maintenance base;
- resolve conflicts by preserving the maintenance branch's package and release
  authority rather than copying `main` wholesale;
- retain `monospace:style=Regular` as the application default while keeping an
  explicit user `main.font` authoritative;
- derive style candidates from the resolved regular family and reuse regular
  safely when a compatible style is unavailable; and
- preserve immutable renderer resources for already-open Windows.

Rerun the original isolated Fontconfig fixtures and focused font tests on the
actual maintenance candidate. Prior evidence supports the design but does not
substitute for validation after backport.

**Gate:** CaskaydiaMono and regular-only fixtures pass on the exact Beta 2 base,
with no hardcoded JetBrains primary-style authority.

### Milestone 5 — documentation and release preparation

Expected files include:

- `README.md`
- `RELEASE_NOTES.md`
- `docs/configuration.md`
- `docs/usage.md`
- `docs/cli.md`
- `docs/status.md`
- corresponding public-site documentation
- `Cargo.toml` and `Cargo.lock`
- `packaging/PKGBUILD`
- `packaging/aur/PKGBUILD` and `.SRCINFO`
- `packaging/aur-bin/PKGBUILD` and `.SRCINFO`
- release-tool fixtures and expected metadata

Work:

- document the two settings, defaults, exact governed paths, persistent
  exceptions, tab-promotion behavior, close consequences, and recovery limits;
- add a migration note that omission preserves Beta 1 behavior;
- describe the startup font fix separately from deferred live font reload;
- bump the workspace to `0.1.0-beta2`, Arch recipes to `0.1.0beta2-1`, and the
  release tag contract to `v0.1.0-beta2`;
- update all package URLs, source identities, generated metadata, release notes,
  status surfaces, and candidate-tool fixtures consistently; and
- keep candidate creation, promotion, AUR distribution, installation, and
  publication as later approval-gated operations.

**Gate:** one exact commit contains internally consistent source, package,
documentation, and release metadata without creating a tag or release.

## Validation

Run focused checks after each coherent milestone and the serialized complete
boundary before release review:

```bash
cargo test -p splinterm-core
cargo test -p splinterm-protocol
cargo test -p splinterm config::tests
cargo test -p splinterm --lib --bin splinterm
cargo test -p splinterd
cargo test --workspace -- --test-threads=1
cargo fmt --all --check
cargo clippy --workspace --all-targets -- -D warnings
python -m pytest tools/package tools/release
python tools/check-foot-provenance.py --mode portable
git diff --check
```

Also run the repository's automation contract fixtures, package integration
suite, source-archive verification, AUR metadata checks, documentation build,
and link checker on the exact candidate tree. Do not broaden tolerances, skip
failed expensive checks without diagnosis, or treat isolated retries as if the
original complete boundary never failed.

## Non-graphical implementation evidence (2026-08-19)

- Reconciled `maint/0.1` to the published Beta 1 runtime and release/status
  state without replaying duplicate runtime patches; bounded review found and
  then confirmed correction of stale Alpha3.3 documentation.
- Added strict backward-compatible configuration defaults, one shared graphical
  launch classifier, commandless trusted transient shells, per-Lair Window
  owner connections, and recency exclusion for unpromoted Lairs.
- Added owner-only atomic promotion for Dojo creation and explicit Dojo rename.
  Core and daemon tests cover one-revision commit, wrong-owner rejection, stale
  revision, persistence failure rollback, lease retention on failure, lease
  removal on success, multi-tab transient cleanup, and survival after promoted
  owner disconnect.
- Kept remote, automation, MCP, named, command-bearing native, preset,
  restore/relaunch, and non-graphical creation persistent. False promotion fields
  remain absent on the wire for backward-readable ordinary request shapes.
- Backported startup font correction `27321e0` and reran focused family/style,
  regular fallback, system `monospace`, and configuration tests on the Beta 2
  maintenance tree.
- The complete serialized workspace passed. Workspace all-targets Clippy passed
  with warnings denied; formatting and `git diff --check` passed.
- Release/package/provenance Python tests passed with one expected
  archive-dependent skip; portable Foot provenance passed after the version-only
  lock digest update.
- The documentation site passed Astro check/build with zero diagnostics and 560
  local links checked.
- Clean committed `0.1.0beta2-1` core and MCP split packages built with their
  full `check()` boundary and passed archive-content plus extracted MCP runtime
  validation. No package was installed.
- One complete pre-release MCP stdio run encountered the existing
  timing-sensitive notification/cancellation assertion; its exact isolated
  retry passed, and later serialized workspace and clean-package runs passed the
  same case.
- Final independent implementation review approved the complete source and
  bounded post-graphical naming fix with no blocker or fix worth doing now.
- Candidate workflow dispatch, protected promotion, AUR publication,
  installation, and forward-port remain pending and separately gated.

## Graphical acceptance

Graphical testing requires one separate approval for the complete guarded matrix
under the repository's workspace 8 / DP-2 isolation and cleanup rules. Test an
adjacent staged package on a private socket before considering any system
installation.

The approved matrix must cover:

1. omitted settings preserve persistent Beta 1 launch behavior;
2. `persistent-by-default=no` makes desktop/`SUPER+ENTER`, bare launch, picker
   New, and in-Window New Terminal client-bound;
3. closing each unpromoted Window terminates and removes only its owned Lairs;
4. two transient Lairs in one Window retire independently;
5. adding a tab promotes when `persist-on-tab-organization=yes`;
6. explicitly naming or renaming a tab promotes when that setting is `yes`;
7. tab creation and naming leave the Lair transient when the setting is `no`;
8. named, command-bearing, preset, remote, automation, and MCP creation remain
   persistent;
9. promoted Lairs survive close, appear in Recent Dojos, and reopen correctly;
10. CaskaydiaMono regular/bold/italic/bold-italic render in newly opened Windows;
11. a regular-only family opens with bounded warnings and regular-face reuse;
    and
12. existing Windows do not claim live font-family replacement.

Record source commit, package archive hash, executable member hashes, client and
daemon device/inode identities, private socket, config, font inputs, topology
before/after, process cleanup, screenshots, original focus/workspace/geometry,
and restored state.

Abort immediately on wrong-window input, unrelated process or Window mutation,
an unowned Lair surviving, a persistent Lair being reaped, partial promotion,
font-family mismatch, or incomplete cleanup.

Acceptance passed on 2026-08-19 at `e6dba77` using the staged
`0.1.0beta2-1` package, private daemon state/config/socket, and isolated
workspace 8 / DP-2 Windows. The matrix covered all four ordinary graphical
creation paths, two independently owned transient Lairs in one Window, both
promotion triggers, promotion disabled, named and native command-bearing
persistence, command-bearing XDG transience, Caskaydia family resolution, and
regular-only Audiowide fallback. Non-graphical boundaries retain the preset,
remote, automation, MCP, restore, and relaunch exceptions.

The matrix exposed a generated-name collision between initial and immediate
in-Window Lair creation. Both sites now use Unix nanoseconds plus PID; the exact
rebuilt package then created and retired two distinct transient Lairs. All test
Windows and private processes were removed, workspace 8 was empty, and baseline
focus/workspace/monitor/geometry were restored. No installed package or user
configuration changed. Exact identities, case outputs, original screenshots,
and checksums are retained in
[`artifacts/0044-beta2-graphical-acceptance/`](artifacts/0044-beta2-graphical-acceptance/).

## Review and integration order

1. Reconcile the Beta 2 maintenance base in one coordinator branch.
2. Implement Milestones 1–3 as dependency-ordered branches with one writer per
   worktree and fresh review at each coherent boundary.
3. Backport and revalidate the font hotfix in a separate branch.
4. Integrate the accepted runtime slices into `maint/0.1`.
5. Prepare version, package, documentation, and candidate metadata in a dedicated
   release-boundary branch.
6. Run complete non-graphical validation and fresh independent review.
7. Request one bounded graphical-test approval and execute the staged package
   matrix.
8. Construct a private release candidate through the manual workflow.
9. Review the closed candidate manifest and artifacts.
10. Request separate approval before protected promotion, AUR publication,
    installation, or any other external mutation.
11. Record the published release and distribution receipts.
12. Forward-port the accepted runtime and documentation changes to `main`
    through a separate reviewed branch without importing stale 0.1 package
    metadata into the active 0.2 line.

Do not combine implementation, graphical acceptance, release promotion, AUR
publication, local installation, and forward-port into one approval or one
branch.

## Completion criteria

Beta 2 is complete only when:

- the exact settings and precedence rules above are implemented and documented;
- every ordinary unnamed local graphical entry point uses one shared lifetime
  decision;
- every persistent exception remains persistent;
- transient ownership and atomic promotion pass focused race/failure tests;
- the startup font hotfix passes again on the reconciled maintenance base;
- complete non-graphical and approved graphical boundaries pass;
- fresh review has no unresolved blocker or fix worth doing now;
- the protected workflow publishes immutable `v0.1.0-beta2` artifacts from the
  reviewed candidate without rebuilding;
- AUR recipes point to and verify those exact assets;
- status and release notes record the real publication state; and
- the separately reviewed forward-port lands without changing 0.2 release
  authority.
