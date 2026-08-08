# Splinterm Product Requirements Document

- **Status:** Draft
- **Product maturity:** Advanced private prerelease
- **As of:** 2026-08-07
- **Product authority:** Current implementation, accepted plans, retained validation evidence, and [Architecture](architecture.md)
- **Related documents:** [Roadmap](roadmap.md), [Product positioning](plans/0021-product-positioning-and-public-documentation.md), [Configuration](configuration.md), [Automation](automation.md), and [Packaging](packaging.md)

## 1. Purpose

This document consolidates Splinterm's product intent and requirements from the current implementation and repository documentation. It defines what the product is for, whom it serves, which outcomes it must preserve, what is implemented today, and what remains before a public release.

This PRD is not an implementation plan or architecture decision log:

- this document owns product outcomes, priorities, and release requirements;
- [architecture.md](architecture.md) owns system boundaries and technical invariants;
- [ADRs](adr/) own accepted architectural decisions;
- [plans](plans/) own dependency-ordered implementation work and evidence gates;
- specialist documents own detailed user and compatibility contracts; and
- accepted plans and retained evidence remain authoritative for whether a feature has been validated.

When this draft conflicts with the current implementation or an accepted ADR, treat the conflict as a product-documentation defect to resolve rather than silently changing behavior.

## 2. Product definition

> **Splinterm is a persistent, security-conscious terminal substrate for humans and bounded automation.**

Splinterm combines a native Wayland terminal with a headless daemon that keeps shells, layouts, terminal state, and scrollback alive when graphical clients disconnect. Humans use the persistent topology through native windows, tabs, panes, search, clipboard, and explicit control workflows. Authorized tools use the same topology through bounded JSON/NDJSON, SSH relay, and MCP interfaces.

Splinterm is built in Rust from Foot's terminal behavior and is designed first for Omarchy and Arch Linux.

### 2.1 Core differentiator

A normal terminal window owns the shell it presents. Splinterm separates those lifetimes:

- `splinterd` owns persistent terminal processes and canonical state;
- `splinterm` is a disposable native Wayland view;
- automation clients are disposable, independently authorized views and controllers; and
- closing or crashing a graphical client must not terminate its daemon-owned shells.

Persistence is the product foundation, not an optional multiplexer feature layered onto a conventional terminal.

### 2.2 Product vocabulary

| Term | Product meaning |
|---|---|
| **Topology** | The daemon's complete persistent session catalog. |
| **Lair** | A named persistent session or project. |
| **Dojo** | A persistent terminal layout inside a Lair. |
| **Splint** | One terminal pane and process slot inside a Dojo. |
| **Window** | A disposable native Wayland client surface. |
| **Tab** | A Window-local attachment to a Dojo; it does not own or persist the Dojo. |

## 3. Problem statement

Terminal-centric work has several recurring failures:

1. closing or losing a graphical terminal can end valuable shell and process state;
2. external multiplexers add a separate interaction and configuration model;
3. graphical use and automation often operate through unrelated state models;
4. automation access is frequently either too weak to be useful or too broad to be trusted;
5. remote access can accidentally turn a local terminal daemon into a network service; and
6. compatibility claims can drift without a pinned behavioral authority.

Splinterm should solve these problems with one daemon-owned topology, a native Omarchy-first interface, explicit and bounded authority, and Foot-derived terminal behavior.

## 4. Target users

### 4.1 Primary users

**Omarchy and Arch developers** who want a fast native Wayland terminal whose shells and layouts survive graphical client closure.

They need:

- familiar terminal behavior;
- native windows, panes, tabs, clipboard, search, IME, scaling, and theme integration;
- quick creation, detachment, reopening, and recovery workflows; and
- honest process-lifetime semantics.

### 4.2 Advanced users and operators

**Users operating persistent local or headless terminal sessions** who need explicit lifecycle, restore, remote access, policy, and troubleshooting controls.

They need:

- a daemon that can run without Wayland;
- owner-controlled policy and service administration;
- authenticated SSH stdio relay without a daemon network listener;
- bounded state, clean shutdown, and documented backup/recovery semantics; and
- clear distinctions between detach, process exit, relaunch, restore, and reset.

### 4.3 Automation and integration authors

**Tool authors and coding-agent operators** who need structured terminal access without receiving trusted graphical authority.

They need:

- versioned JSON/NDJSON schemas;
- stable IDs, revisions, errors, and exit categories;
- least-privileged executable-scoped policy;
- bounded observation and exclusive control;
- explicit resynchronization after gaps; and
- CLI, SSH relay, and MCP paths over the same topology.

### 4.4 Evaluators and contributors

**Evaluators** need to understand the product, differentiator, validated environment, current maturity, and first workflow quickly. **Contributors** need clear architecture, ADRs, plans, test authorities, and scope boundaries.

## 5. Jobs to be done

1. **Keep work alive:** When I close, restart, or lose a graphical terminal client, keep daemon-owned shells and layouts running so I can reconnect later.
2. **Organize terminal work:** Let me arrange persistent sessions, layouts, and panes while using disposable native windows and tabs as views.
3. **Resume deliberately:** Let me create a fresh terminal by default, choose a recent running Dojo, or reopen the last remembered running Dojo without confusing reopening with process restoration.
4. **Control safely:** Let multiple clients observe the same state while ensuring only one controller owns input and resize authority for a Splint at a time.
5. **Automate explicitly:** Let authorized tools inspect and mutate terminal topology through stable machine contracts without inheriting human UI trust.
6. **Work remotely without exposing a daemon port:** Let a human use native remote Splinterm through authenticated SSH, while separately policy-scoped automation uses the raw stdio relay.
7. **Retain terminal compatibility:** Give me terminal behavior grounded in a pinned Foot authority rather than an undocumented approximation.
8. **Fit Omarchy naturally:** Follow the active Omarchy theme, launch through `xdg-terminal-exec`, and behave as a native Wayland application without modifying user configuration automatically.

## 6. Product principles

1. **Daemon-owned persistence:** terminal processes and canonical state outlive graphical views.
2. **One substrate:** humans and authorized automation operate on the same topology.
3. **Bounded automation:** authority is explicit, scoped, revocable, resource-bounded, and independently identified.
4. **Disposable clients:** neither graphical nor automation clients become the source of persistent truth.
5. **Exact targeting:** stale IDs, incarnations, revisions, or captured targets fail explicitly; operations never guess a replacement.
6. **Terminal content is data:** output can never grant authority, approve consent, change policy, or become an automatic instruction.
7. **Compatibility needs an oracle:** Foot 1.27.0 commit `3c5b584b0eafa772eb4376fb6eaf6643399e190e` remains the behavioral authority.
8. **Honest maturity:** implemented, validated, supported, proposed, deferred, and publicly released have distinct meanings.
9. **Omarchy first, portability later:** optimize and validate the primary environment before claiming broader support.
10. **No hidden mutation:** installation and integration do not silently change the default terminal, user keybindings, SSH policy, or Omarchy-owned files.

## 7. Current product baseline

The following table summarizes the current repository state. “Validated” means accepted plans or retained evidence record the required checks; it does not imply a public compatibility guarantee beyond the documented scope.

| Area | Current status |
|---|---|
| Native Wayland terminal | Implemented and validated on the documented Omarchy/Hyprland target. |
| Daemon-owned sessions and client detach/reattach | Implemented and validated. |
| Lair/Dojo/Splint topology and persistent multiplexing | Implemented and validated. |
| Multi-pane graphical layouts | Implemented and validated. |
| Window-local Dojo tabs | Implemented; a Window supports up to 32 distinct Dojo attachments and may mix Lairs. |
| Recent Sessions picker and reopen-last workflow | Implemented. |
| Searchable 31-command palette and trusted tab context menu | Implemented, packaged, installed, reviewed, and validated by Plan 0025. |
| Multi-client observation and exclusive control transfer | Implemented and validated. |
| Scrollback, literal search, selection, clipboard, IME, and scaling | Implemented within the documented contracts. |
| Omarchy theme discovery and live reload | Implemented. |
| Sixel | Supported Foot-compatible bounded implementation. |
| Kitty graphics | Supported practical static-image subset; not full Kitty compatibility. |
| iTerm2 inline images | Supported bounded inline-PNG subset. |
| JSON/NDJSON automation | Implemented as the publicly documented machine compatibility contract; this does not imply public product availability. |
| SSH stdio relay | Implemented and validated; no daemon network listener. |
| Native remote graphical transport | Phases 1–2 implemented and non-graphically validated: strict profiles, one-authentication multiplexer, endpoint-bound human-interactive workflow, remote-safe launches, namespaced recency, remote no-image/focus enforcement, lifecycle, and non-mutating check. Real-host graphical evidence remains Phase 4. |
| MCP adapter | Implemented and validated as an optional separately identified package. |
| Arch/Omarchy package and edge installer | Private prerelease package and installation path implemented and validated. |
| Public distribution and support policy | Not released. |
| Nix and broader distribution | Planned. |
| Public product/documentation website | Proposed and in active repository work; not a released product surface. |

## 8. Goals

### 8.1 Current product goals

- Preserve shell, layout, terminal, and scrollback continuity across graphical client disconnection.
- Provide a complete native Wayland terminal workflow for the validated Omarchy environment.
- Make persistent multiplexing understandable through Lairs, Dojos, Splints, windows, panes, and tabs.
- Offer first-class human workflows without requiring an external multiplexer.
- Provide stable, bounded machine access through CLI, relay, and MCP contracts.
- Keep human consent, trusted UI, automation policy, and control ownership visibly distinct.
- Preserve Foot-derived behavior with reproducible differential evidence.
- Keep ordinary text-only terminal use efficient and ensure optional image support has bounded resource cost.
- Package and upgrade the private prerelease without silently changing user-owned desktop configuration.
- Establish clear current-status, usage, CLI, security, and release documentation before public distribution.

### 8.2 Public-release goals

A public release should let a supported user:

1. understand the product and maturity before installation;
2. install from a versioned, immutable, verifiable artifact;
3. launch a fresh terminal through the documented desktop/XDG path;
4. detach and reconnect without losing daemon-owned work;
5. use panes, tabs, history, clipboard, search, and control workflows confidently;
6. understand persistence and upgrade limitations before risking active work;
7. configure the supported surface and migrate documented Foot values;
8. authorize or decline automation using least-privileged examples; and
9. find a declared compatibility, upgrade, troubleshooting, and support policy.

## 9. Non-goals

The current product does not promise:

- production readiness, stability, or public support;
- transparent process continuity across daemon crash, package upgrade, logout, reboot, or host failure;
- restoration of process memory, kernel PTYs, or arbitrary unpersisted state;
- full Foot configuration compatibility;
- tmux configuration or plugin compatibility;
- broad Linux, compositor, or distribution support;
- a daemon TCP listener or unauthenticated remote API;
- collaborative simultaneous typing or automatic controller stealing;
- automation access to trusted graphical authority or compositor window control;
- semantic coding-agent task status, inter-agent messaging, readiness, completion, or result transport;
- terminal output as trusted instructions;
- full Kitty graphics compatibility, external Kitty file/SHM transports, placeholders, relative placement, or animation;
- arbitrary shell/plugin/terminal-content commands in trusted application menus;
- a general graphical widget toolkit; or
- automatic edits to Hyprland, Omarchy, SSH, terminal preference, or user policy files.

## 10. Functional requirements

Priority meanings:

- **P0:** defining behavior or release blocker;
- **P1:** required for a complete supported primary workflow;
- **P2:** valuable expansion that must not weaken P0/P1 contracts.

### 10.1 Persistence and topology

| ID | Priority | Requirement | Current state |
|---|---:|---|---|
| `FR-PERSIST-01` | P0 | `splinterd` must remain the sole owner and writer of live terminal processes, canonical terminal state, topology, and durable metadata. | Implemented |
| `FR-PERSIST-02` | P0 | Closing or crashing a graphical client must not terminate daemon-owned Splints. | Implemented and validated |
| `FR-PERSIST-03` | P0 | The product must model stable Lair, Dojo, and Splint identities independently of names, display position, and client focus. | Implemented |
| `FR-PERSIST-04` | P0 | Relaunch must retain the Splint identity while allocating a new nonzero process incarnation; stale authority must not carry across incarnations. | Implemented |
| `FR-PERSIST-05` | P0 | After daemon or host loss, restore must recover only validated topology and launch metadata with leaves exited/restorable and relaunch remaining explicit. It must not claim restoration of live PTYs, process memory, terminal grids, scrollback bodies, or image bodies. | Implemented; documentation must remain explicit |
| `FR-PERSIST-06` | P1 | Structural mutations must be revision-bound and fail rather than overwrite concurrent topology changes. | Implemented |

### 10.2 Native human interface

| ID | Priority | Requirement | Current state |
|---|---:|---|---|
| `FR-UI-01` | P0 | Provide a native Wayland terminal client with keyboard, pointer, clipboard, IME, scaling, and damage-driven rendering in the validated environment. | Implemented and validated |
| `FR-UI-02` | P0 | A managed Window must render and operate a persistent Dojo containing a binary Splint layout. | Implemented |
| `FR-UI-03` | P1 | A Window must support an ordered, non-persistent set of no more than 32 distinct Dojo tabs, including Dojos from multiple Lairs. | Implemented |
| `FR-UI-04` | P0 | Closing a tab must detach only the Window-local reference; it must not close the Dojo or terminate its Splints. | Implemented |
| `FR-UI-05` | P1 | Hidden tabs must remain synchronized without painting, blinking, reporting focus, resizing, or owning a controller. | Implemented |
| `FR-UI-06` | P1 | Provide trusted Recent Sessions, command palette, tab menu, rename, confirmation, search, consent, and control surfaces that isolate terminal input while active. | Implemented |
| `FR-UI-07` | P0 | Destructive actions must be explicit, exactly targeted, and confirmed where documented; cancellation must be the safe default. | Implemented |
| `FR-UI-08` | P1 | Modal actions must capture exact resource identities and availability when opened and must never retarget because asynchronous state changes. | Implemented |
| `FR-UI-09` | P1 | Native application controls must remain usable without exposing shell-, plugin-, terminal-content-, or automation-provided trusted commands. | Implemented |

### 10.3 Session lifecycle and control

| ID | Priority | Requirement | Current state |
|---|---:|---|---|
| `FR-LIFE-01` | P0 | Default desktop/XDG launch must create a fresh Lair and Dojo rather than silently attaching to previous work. | Implemented |
| `FR-LIFE-02` | P1 | Reopening existing work must be a separate explicit picker or reopen-last action. | Implemented |
| `FR-LIFE-03` | P0 | Kill, close, detach, relaunch, restore, and reset must remain separate operations with honest consequences. | Implemented |
| `FR-CTRL-01` | P0 | Observation must not imply input or resize control. | Implemented |
| `FR-CTRL-02` | P0 | At most one connection may own controller/size authority for a live Splint at a time; different Splints may have different controllers. | Implemented and validated |
| `FR-CTRL-03` | P0 | Control transfer, force transfer, release, revocation, denial, timeout, and disconnect behavior must be explicit and visible. | Implemented |
| `FR-HIST-01` | P1 | Scrollback and literal search must be bounded, revision/generation-aware, and resynchronize explicitly after gaps or replacement. | Implemented |

### 10.4 Configuration and Omarchy integration

| ID | Priority | Requirement | Current state |
|---|---:|---|---|
| `FR-CONFIG-01` | P1 | Support a small explicit configuration surface with strict parsing, bounded values, and actionable diagnostics for unsupported settings. | Implemented |
| `FR-CONFIG-02` | P1 | Support the documented Foot migration subset without claiming arbitrary `foot.ini` compatibility. | Implemented |
| `FR-OMARCHY-01` | P0 | Integrate through stable application identity and `xdg-terminal-exec` without modifying user Hyprland or Omarchy configuration automatically. | Implemented |
| `FR-OMARCHY-02` | P1 | Discover the active Omarchy theme and apply valid palette changes live while retaining the last valid theme during transient or malformed updates. | Implemented |
| `FR-OMARCHY-03` | P1 | Preserve a portable explicit theme override and a safe bundled fallback when Omarchy state is unavailable. | Implemented |

### 10.5 Terminal behavior and images

| ID | Priority | Requirement | Current state |
|---|---:|---|---|
| `FR-TERM-01` | P0 | Terminal semantics and renderer behavior must retain Foot provenance and use the pinned Foot oracle for claimed compatibility. | Implemented and validated within recorded scopes |
| `FR-TERM-02` | P0 | Full-frame, incremental, scroll-copy, cache, resize, and reattach paths must preserve equivalent visible results for equivalent terminal state. | Implemented and validated within accepted plans |
| `FR-IMG-01` | P1 | Image semantics must remain daemon-owned and protocol-independent while pixel bodies use a separate bounded trusted-client transfer path. | Implemented |
| `FR-IMG-02` | P1 | Support the documented bounded Sixel, practical Kitty static-image, and inline iTerm2 PNG subsets without claiming broader compatibility. | Implemented and documented |
| `FR-IMG-03` | P0 | Image support must not enlarge every terminal cell, place pixel bodies in public automation/audit records, or create unbounded decode, cache, queue, or scrollback storage. | Implemented |
| `FR-IMG-04` | P1 | Unsupported external media transports must fail closed rather than granting ambient filesystem or shared-memory authority. | Implemented |

### 10.6 Automation and remote operation

| ID | Priority | Requirement | Current state |
|---|---:|---|---|
| `FR-AUTO-01` | P0 | Provide human-readable CLI behavior and versioned stable JSON/NDJSON machine contracts with checked-in schemas. Human rendering is not a compatibility boundary. | Implemented |
| `FR-AUTO-02` | P0 | Machine records must carry stable operation, resource identity, revision, truncation, error, and resynchronization semantics appropriate to each operation. | Implemented |
| `FR-AUTO-03` | P0 | Machine mode must never prompt; destructive actions must require explicit confirmation flags or remain outside the machine contract. | Implemented |
| `FR-AUTO-04` | P0 | Automation topology mutation must not imply native Window mapping, compositor focus, movement, resize, or workspace control. | Implemented and documented |
| `FR-AUTO-05` | P1 | Provide a separately executable and independently policy-identifiable MCP adapter with the documented operation coverage. | Implemented and validated |
| `FR-REMOTE-01` | P0 | Remote operation must use SSH relays; `splinterd` must expose no network listener by default. Raw automation relay access remains exact-policy scoped. | Implemented and validated |
| `FR-REMOTE-02` | P0 | Machine clients must not gain human terminal authority from SSH login, socket access, same-UID execution, or being inside a Splint. | Implemented |
| `FR-REMOTE-03` | P0 | A native remote client must use one authenticated OpenSSH process carrying bounded independent daemon channels rather than one login per pane or a daemon network listener. | Implemented through native workflow; real-host graphical evidence pending |
| `FR-REMOTE-04` | P0 | Every SSH graphical channel must negotiate the human `RemoteInteractive` role without automation policy, while suppressing trusted-local focus, image, and forced-transfer authority. | Implemented and non-graphically validated; real-host graphical evidence pending |
| `FR-REMOTE-05` | P0 | OpenSSH must retain authority for keys, agents, passwords, passphrases, askpass, and host keys; Splinterm must store no credentials or accept unknown keys automatically. | Implemented in Phase 1 transport lifecycle |
| `FR-REMOTE-06` | P0 | Disconnecting the local remote client must release connection-owned authority without terminating daemon-owned remote Splints. | Transport cleanup implemented; real-host proof remains Phase 4 |
| `FR-REMOTE-07` | P0 | A native remote Window must create, attach, and control new Lairs, Dojos, and Splints immediately without policy publication or reopening. | Implemented and non-graphically validated; real-host graphical validation pending |

### 10.7 Packaging and installation

| ID | Priority | Requirement | Current state |
|---|---:|---|---|
| `FR-PKG-01` | P0 | Private packages must contain coherent adjacent runtime executables so trusted graphical identity checks cannot be bypassed by a shadowing client. | Implemented and validated |
| `FR-PKG-02` | P0 | Installation and upgrade must verify exact artifacts, warn before ending daemon-owned shells, and report the lack of cross-version process continuity. | Implemented |
| `FR-PKG-03` | P0 | Packaging must not edit user homes, default terminal preference, Omarchy-owned files, SSH policy, or service lingering without an explicit separate action. | Implemented |
| `FR-PKG-04` | P1 | The MCP adapter must remain an optional exact-version split package and installation alone must grant no authority. | Implemented |
| `FR-PKG-05` | P0 | Public distribution must use immutable versioned source/artifact URLs, checksums, and a documented upgrade/support policy. | Not yet released |

## 11. Security and privacy requirements

| ID | Priority | Requirement |
|---|---:|---|
| `SEC-01` | P0 | Default third-party authority must fail closed. No policy file means no persistent third-party grant. |
| `SEC-02` | P0 | Grants must bind explicit scopes, peer and executable identity, exact Splint/incarnation, and bounded lifetime semantics. |
| `SEC-03` | P0 | Revocation must release related controllers and subscriptions. |
| `SEC-04` | P0 | Consent UI must be trusted application chrome and must not render requester-controlled terminal content. |
| `SEC-05` | P0 | Terminal, scrollback, clipboard, search, and input bodies must not appear in audit metadata. |
| `SEC-06` | P0 | Terminal-derived data and in-Splint context variables are discovery hints only, never credentials or instructions. |
| `SEC-07` | P0 | Commands must carry structured argv and working-directory fields; implementations must not rebuild arguments into a shell string. |
| `SEC-08` | P0 | Messages, queues, subscriptions, history, searches, images, transfers, and caches must have explicit bounds and deadlines. |
| `SEC-09` | P0 | Stale identities, revisions, cursors, tokens, and incarnations must fail explicitly without fallback selection. |
| `SEC-10` | P0 | Public claims must say “security-conscious” and describe concrete controls; they must not claim absolute security. |

Detailed scope and policy behavior remain authoritative in [automation.md](automation.md), [ADR 0005](adr/0005-trusted-consent-broker.md), and [ADR 0007](adr/0007-supported-automation-policy.md).

## 12. Non-functional requirements

### 12.1 Reliability

- A graphical-client failure must not end daemon-owned Splints.
- A slow observer must not block PTY consumption or topology mutation.
- Subscription gaps must be detected and surfaced as explicit resynchronization requirements.
- Shutdown must drain owned connection tasks, reap children, persist final metadata where promised, and remove runtime sockets cleanly.
- Failed spawn, persistence, or mutation operations must not leave addressable phantom state.

### 12.2 Performance and resource bounds

- Ordinary terminal use must remain responsive under the accepted output, resize, history, and multi-pane matrices.
- Idle behavior must remain event-driven; optional features must not add material idle work when unused.
- Memory ownership and high-water behavior must be measurable for terminal history, publication queues, image content, transfers, and renderer caches.
- Performance work must preserve correctness, bounded memory, small-write latency, and resynchronization contracts.
- Quantitative gates and retained evidence belong in the relevant accepted plan and benchmark documentation rather than being duplicated here.

### 12.3 Compatibility

- Foot 1.27.0 at the pinned commit is the terminal behavior and differential-test authority.
- The validated primary platform is x86_64 Arch/Omarchy with native Wayland under the documented Hyprland environment.
- Broader compositor, distribution, Nix, sandboxed-package, or hardware claims require separate implementation and evidence.
- Public JSON/NDJSON schemas and documented operation semantics are compatibility boundaries; private daemon frames and Rust types are not.

### 12.4 Accessibility and input

- Trusted overlays must provide keyboard operation, visible focus/selection, safe cancellation, and pointer isolation.
- Selection must not rely on color alone.
- IME/preedit, compose fallback, fractional scale, focus indication, and reduced-motion behavior must remain functional within the accepted native-client scope.
- Destructive confirmation must default to cancellation.

### 12.5 Maintainability

- Domain types must remain independent of rendering, Wayland, PTYs, async runtimes, and wire formats as documented by crate boundaries.
- Graphical clients, automation clients, and transport adapters must remain replaceable without becoming persistent-state authorities.
- Every significant architecture change must be recorded in an ADR; implementation slices must retain focused validation evidence.

## 13. User experience requirements

### 13.1 First-run workflow

A supported user should be able to:

1. install a verified package;
2. launch a fresh terminal through the desktop or XDG path;
3. create panes and Dojos using discoverable native controls;
4. close or detach the graphical view without ending the shell;
5. reopen the running Dojo from Recent Sessions or reopen-last; and
6. understand when an operation will terminate a process or lose state.

### 13.2 Interaction rules

- Fresh launch and reopen must remain distinct.
- Window/tab focus is client-local and must not move another client's focus.
- Tab close is non-destructive detach; Splint/Dojo termination is explicitly destructive.
- Unavailable actions remain visibly unavailable rather than silently selecting another target.
- Trusted surfaces consume relevant input and prevent click-, paste-, IME-, focus-, selection-, URL-, and terminal-mouse-through.
- The command palette remains a closed application-owned catalog; terminal output and plugins cannot add trusted actions.
- Error messages must identify stale state, denied authority, unsupported behavior, and recovery steps without exposing sensitive bodies.

## 14. Success criteria

### 14.1 Product success

Splinterm meets its defining product promise when:

- daemon-owned shells and layouts remain live across graphical client disconnect and reconnect;
- a user can complete the primary Omarchy terminal workflow without an external multiplexer;
- native windows, panes, tabs, history, search, clipboard, control, and theme behavior pass their accepted validation gates;
- automation can inspect and operate the same topology through explicit least-privileged policy;
- no automation path receives trusted graphical authority by implication;
- Foot-derived behavior and intentional divergences remain reproducible and documented; and
- optional capabilities remain bounded and do not regress ordinary terminal use.

### 14.2 Public-release readiness

Public release is blocked until all of the following are true:

1. a current status document defines supported environments, validated capabilities, known limitations, deferred work, and open gates;
2. product, usage, CLI, installation, configuration, security, troubleshooting, and automation documentation have clear authoritative homes;
3. README, website, package, desktop, AppStream, and Cargo descriptions use consistent product and maturity language;
4. versioned immutable public artifacts and checksums are available;
5. install, upgrade, daemon restart, active-shell loss, rollback, and compatibility expectations are documented and validated;
6. a public compatibility and support policy exists;
7. machine schemas and examples are published with their compatibility rules;
8. licensing and Foot provenance are complete in source and packages;
9. all release-blocking non-graphical and approved graphical matrices pass on a clean committed build; and
10. independent product/readability and technical-accuracy reviews have no unresolved blockers.

### 14.3 Documentation comprehension

A new evaluator should be able to answer within one minute:

- What is Splinterm?
- Why does daemon-owned persistence matter?
- How do humans and bounded automation share one topology?
- Which environment and capabilities are validated?
- Why is it still a private prerelease?
- What is the first safe workflow?

## 15. Risks and mitigations

| Risk | Product impact | Mitigation |
|---|---|---|
| Persistence is mistaken for crash/reboot resurrection | Users risk losing active work | Use explicit lifetime language at install, upgrade, restore, and shutdown boundaries. |
| Automation is marketed as unrestricted “AI control” | Authority and trust boundaries become unclear | Use “bounded automation”; document identity, scopes, revocation, control, and untrusted output. |
| Product documentation drifts behind accepted implementation | Users see contradictory maturity and feature claims | Establish one status authority and link specialist documents instead of duplicating claims. |
| Foot compatibility claims exceed evidence | Applications behave unexpectedly | Keep the pinned oracle, exact scope language, and differential gates. |
| Multiplexer concepts become harder than tmux | Primary workflow becomes inaccessible | Lead with user outcomes, native controls, clear vocabulary, and discoverable trusted menus. |
| Optional images or history regress ordinary use | Core terminal responsiveness and memory suffer | Preserve explicit budgets, no-image gates, event-driven expiry, and benchmark matrices. |
| Trusted UI and terminal content blur together | Spoofing or accidental authority | Keep trusted chrome visually distinct and input-isolated; never derive authority from terminal content. |
| Private packaging is mistaken for public support | Unsupported users depend on unstable upgrade behavior | Keep advanced-private-prerelease labeling prominent until release gates are met. |
| Platform expansion dilutes the validated Omarchy path | More environments than the project can test | Require separate evidence and support decisions for each platform. |

## 16. Open product decisions

These decisions remain outside the current accepted product baseline:

1. public release versioning, channels, and support duration;
2. exact public compatibility guarantees beyond current machine schemas and documented subsets;
3. Nix, Home Manager, and tertiary-distribution scope;
4. compositor support beyond the validated Hyprland/Omarchy environment;
5. sandboxed packaging and daemon/socket integration;
6. whether and how to support advanced Kitty graphics, placeholders, external media, or animation;
7. whether client-native compositor/window automation receives a separate trusted graphical broker;
8. public issue/support/security-reporting processes;
9. telemetry policy—currently no telemetry requirement is defined; and
10. stable remappable application keybindings and any safe extension model for trusted commands.

Each material decision should receive an ADR or a scoped product/implementation plan before changing compatibility or authority boundaries.

## 17. Documentation authority map

| Subject | Authority |
|---|---|
| Product requirements and priorities | This PRD after review and acceptance |
| Current maturity and availability | Planned `docs/status.md`; until then, accepted plans plus current implementation |
| Architecture and ownership | [architecture.md](architecture.md) |
| Architecture decisions | [docs/adr/](adr/) |
| Roadmap and future phases | [roadmap.md](roadmap.md) |
| Human configuration and bindings | [configuration.md](configuration.md) |
| Machine contracts and policy | [automation.md](automation.md) |
| MCP integration | [mcp.md](mcp.md) |
| SSH remote access | [remote.md](remote.md) |
| Headless operation | [headless.md](headless.md) |
| Terminal images | [images.md](images.md) |
| Packaging and installation | [packaging.md](packaging.md) |
| Integration-author workflows | [integrations.md](integrations.md) |
| Implementation sequencing and evidence | [docs/plans/](plans/) and [docs/spikes/](spikes/) |
| Foot lineage and licenses | [ADR 0001](adr/0001-foot-rust-port.md), [THIRD_PARTY.md](../THIRD_PARTY.md), and [LICENSE](../LICENSE) |

## 18. Source documents used for this draft

This draft synthesizes the current implementation and, principally:

- [README](../README.md)
- [Architecture](architecture.md)
- [Pre-planning research](pre-planning-research.md)
- [Roadmap](roadmap.md)
- [Plan 0001: terminal kernel](plans/0001-terminal-kernel.md)
- [Plan 0002: Omarchy-native terminal MVP](plans/0002-omarchy-terminal-mvp.md)
- [Plan 0004: persistent multiplexing](plans/0004-phase3-multiplexing.md)
- [Plan 0006: headless automation](plans/0006-phase4-headless-automation.md)
- [Plan 0007: MCP adapter](plans/0007-phase4-mcp-adapter.md)
- [Plan 0008: terminal image protocols](plans/0008-terminal-image-protocols.md)
- [Plan 0021: product positioning](plans/0021-product-positioning-and-public-documentation.md)
- [Plan 0025: command palette and tab menus](plans/0025-command-palette-and-tab-context-menus.md)
- [Plan 0026: public website and documentation site](plans/0026-public-website-and-documentation-site.md)
- [Supported automation contracts](automation.md)
- [Configuration and Foot migration](configuration.md)
- [Private packaging](packaging.md)
- [Terminal image compatibility](images.md)

## 19. Draft review questions

Before accepting this PRD, reviewers should decide:

1. Is “persistent, security-conscious terminal substrate for humans and bounded automation” the accepted product definition?
2. Are Omarchy/Arch developers the correct primary user, with operators and integration authors as secondary users?
3. Does the current baseline accurately distinguish implemented, validated, supported, proposed, deferred, and unreleased behavior?
4. Are the P0 requirements truly release-blocking?
5. Does any requirement accidentally promise daemon-crash, reboot, or upgrade continuity?
6. Does any automation language imply trusted graphical or compositor authority?
7. Are any current capabilities missing, or are implementation details incorrectly elevated to product requirements?
8. Which public-release gates require a product decision rather than more implementation?
9. Should `docs/status.md` be created before this PRD is accepted as an authority?
10. Which success criteria should gain quantitative thresholds before public release?
