# Plan 0021: product positioning and public documentation

- **Status:** Complete — repository authority, human usage, CLI, contributor workflow, history framing, metadata alignment, focused validation, and two independent reviews pass
- **Date:** 2026-08-04
- **Reconciled:** 2026-08-09 against commits `1d8fa51`, `7a44e33`, `1e0a4ad`, and current closure work
- **Product authority:** current implementation, retained validation evidence, and [Architecture](../architecture.md)
- **Related status:** [Roadmap](../roadmap.md), [supported automation](../automation.md), and [private packaging](../packaging.md)
- **Documentation scope:** public positioning, maturity language, README information architecture, and distribution metadata

## Reconciliation record — 2026-08-09

The original `Proposed` label became stale after `1d8fa51` rewrote the README
around the accepted product sentence, persistence, bounded automation, validated
platform, and advanced-private-prerelease maturity. Commit `7a44e33` added the
PRD and a buildable public documentation site with status, installation,
quickstart, sessions, configuration, troubleshooting, concepts, and development
content. The current site passes Astro diagnostics/build and its local-link
validator; desktop, AppStream, and PKGBUILD metadata syntax also pass their
focused validators.

The closure work establishes [`docs/status.md`](../status.md) as the repository
maturity authority with a verified capability truth table, limitations, release
gates, validated environment, and one-authority-per-subject map. Detailed human
operation and CLI references now live in [`docs/usage.md`](../usage.md) and
[`docs/cli.md`](../cli.md); contributor documentation covers isolated daemons,
validation, benchmarks, the pinned Foot oracle, fuzzing, graphical guardrails,
and packaging safety.

README links now lead to repository authority, the roadmap and pre-planning
research have current/archival framing, and package/desktop/AppStream/selected
Cargo descriptions use the approved short product sentence. The PRD no longer
calls the implemented website proposed and names the current authority files.
Website copies remain summaries rather than silently replacing repository
sources. Focused validation passes. Product/readability reviewer `93d8c0d2`
concluded **may mark complete** without a finding. Technical reviewer `fb9f72d9`
identified one inaccurate blanket prompt claim; the usage/CLI authorities now
match the dispatcher contract exactly, post-review validation passes, and the
[review disposition](artifacts/0021-product-positioning/closure-2026-08-09/review/disposition.md)
records no remaining blocker.

## Decision

Position Splinterm as:

> **A persistent, security-conscious terminal substrate for humans and bounded
> automation.**

Use **advanced private prerelease** as the consistent product-maturity label.
This communicates that substantial behavior is implemented and validated while
public distribution, compatibility guarantees, and a support policy remain
unreleased.

Remove “proposed” from descriptions of the existing product. Reserve that word
for plans and capabilities that are not implemented.

The README must lead with product identity, differentiation, current capability,
and honest availability rather than implementation history, protocol versions,
or milestone evidence. Detailed operational and technical material should move
to authoritative documentation instead of being duplicated in the README.

## Problem

The current README opens by calling Splinterm “proposed,” then immediately
describes a validated private prerelease with completed terminal, persistence,
multiplexing, automation, packaging, and interoperability phases. This creates
several conflicting readings:

- research prototype;
- advanced prerelease;
- personal daily-use terminal; or
- emerging distributable product.

The implementation has progressed beyond a proposal, but the public language
has not established a simple distinction between implementation maturity and
release availability.

The README is also information-dense. Its first page mixes product vocabulary,
workspace structure, implementation history, validation status, protocol
versions, daily workflows, a complete CLI cookbook, keyboard bindings,
persistence semantics, renderer behavior, security boundaries, image support,
packaging, development commands, and research lineage. This obscures the
project’s strongest differentiator:

> Persistent terminal state owned by a headless daemon, exposed both to humans
> and to explicitly bounded automation.

## Goals

- Give Splinterm one clear product sentence.
- Distinguish implemented and validated behavior from public-release status.
- Make the README useful to a new user before serving contributors and protocol
  authors.
- Protect persistence, shared human/automation access, explicit authority, and
  Foot-derived compatibility as the primary differentiators.
- Replace phase and protocol narration with a concise current-capability view.
- Establish one authoritative status document and eliminate duplicated maturity
  paragraphs.
- Move detailed human usage, CLI, and development material into appropriate
  documentation.
- Align README, roadmap, package, desktop, AppStream, and Cargo descriptions.
- Preserve honest security and compatibility boundaries without weakening them
  for simpler marketing language.

## Non-goals

- Declaring Splinterm production-ready, stable, or publicly supported.
- Publishing packages, repositories, releases, or an AUR submission.
- Claiming a personal daily-driver status without explicit recorded evidence.
- Claiming broad Linux, compositor, or distribution support beyond validated
  environments.
- Rebranding Splinterm as an “AI terminal.”
- Implying that automation receives trusted graphical authority or native Window
  control.
- Rewriting historical plans, ADRs, or evidence to use current marketing
  language.
- Removing technical detail from its authoritative specialist documentation.
- Changing implementation, protocol, packaging behavior, or security policy.
- Running graphical tests.

## Positioning system

### Primary product sentence

Use this sentence, or a reviewed meaning-preserving refinement, across the
README and public product metadata:

> **Splinterm is a persistent, security-conscious terminal substrate for humans
> and bounded automation.**

“Substrate” must be followed by a plain-language explanation for readers who do
not yet know the architecture:

> It combines a native Wayland terminal with a headless daemon that keeps
> shells, layouts, and scrollback alive when graphical clients disconnect.
> Humans use the same persistent topology through native windows, panes, and
> tabs; authorized tools access it through bounded JSON/NDJSON, SSH relay, and
> MCP interfaces.

### Supporting platform sentence

> Splinterm is built in Rust from Foot’s terminal behavior and is designed first
> for Omarchy and Arch Linux.

Platform identity supports the product statement but should not displace
persistence and bounded automation as the lead differentiator.

### Maturity sentence

> **Status: advanced private prerelease.** Core terminal emulation, persistent
> sessions, multiplexing, native Wayland presentation, Arch packaging, and
> bounded automation workflows are implemented and validated. Public
> distribution, compatibility guarantees, and a support policy have not yet
> been released.

### Interpretation

“Advanced private prerelease” means:

- the product runs;
- substantial core paths are implemented;
- required validation evidence exists for named accepted milestones;
- a private package and normal daily-use workflow exist;
- public installation and upgrade support are not promised;
- compatibility and support policies are not final; and
- future plans remain proposals rather than current product behavior.

## Messaging hierarchy

Every top-level public surface should answer these questions in order:

1. **What is it?** A persistent Wayland terminal and security-conscious
   automation substrate.
2. **Why is it different?** Sessions belong to a headless daemon rather than a
   graphical window, and humans and bounded automation operate over the same
   topology.
3. **Does it work?** Core terminal, persistence, multiplexing, graphical,
   packaging, and automation paths are implemented and validated.
4. **Is it a supported public release?** No. It is an advanced private
   prerelease.
5. **How can it be evaluated?** Provide concise environment, installation, and
   first-workflow instructions.

Implementation history, protocol versions, evidence links, and contributor
commands follow these answers rather than preceding them.

## Differentiators to protect

### Persistence is foundational

`splinterd` owns shells, terminal state, layouts, and durable metadata. A
Wayland Window is a disposable view rather than the owner of terminal process
lifetime.

### Humans and automation share one substrate

Native windows, human CLI commands, JSON/NDJSON clients, the policy-scoped SSH
relay, and the MCP adapter operate over the same persistent topology. They do
not create separate human and automation terminal worlds.

### Automation is bounded

Public language should retain the material controls:

- explicit scopes;
- executable identity;
- bounded resources and messages;
- consent and revocation;
- exclusive controller ownership;
- bounded audit metadata that excludes terminal bodies; and
- terminal output never becoming authority or automatic instruction.

The concise phrase is “bounded automation,” not “AI access” or “agent control.”

### Compatibility has an authority

Foot is Splinterm’s behavioral foundation and pinned oracle, not merely visual
inspiration. Keep the exact provenance in technical documentation and a concise
lineage statement in the README.

## Terminology policy

| Term | Public meaning |
| --- | --- |
| **Implemented** | Present in the current code. |
| **Validated** | Required recorded evidence exists for the named behavior. |
| **Advanced private prerelease** | Current product maturity and availability. |
| **Supported** | A deliberate, documented compatibility contract exists. |
| **Proposed** | Planned or under decision; not current behavior. |
| **Deferred** | Intentionally outside the current product. |
| **Public release** | Not yet reached. |

Avoid these unqualified descriptions:

- secure terminal;
- production-ready;
- stable product;
- AI terminal;
- research prototype;
- proposed project; and
- publicly available.

Prefer “security-conscious” over an absolute security claim. State concrete
controls where security matters.

## Proposed README opening

```markdown
# Splinterm

**Splinterm is a persistent, security-conscious terminal substrate for humans
and bounded automation.**

It combines a native Wayland terminal with a headless daemon that keeps shells,
layouts, and scrollback alive when graphical clients disconnect. Humans use the
same persistent terminal topology through native windows, panes, and tabs;
authorized tools access it through bounded JSON/NDJSON, SSH relay, and MCP
interfaces.

Splinterm is built in Rust from Foot’s terminal behavior and is designed first
for Omarchy and Arch Linux.

> [!IMPORTANT]
> **Status: advanced private prerelease.** Core terminal emulation, persistent
> sessions, multiplexing, native Wayland presentation, Arch packaging, and
> bounded automation workflows are implemented and validated. Public
> distribution, compatibility guarantees, and a support policy have not yet
> been released.
```

The final rewrite may improve cadence but must preserve the product, platform,
and maturity meanings.

## Target README information architecture

```text
# Splinterm
  Product statement
  Plain-language architecture
  Platform sentence
  Maturity callout

## Why Splinterm
  Persistence by architecture
  Human and bounded-automation access
  Explicit authority boundaries
  Foot-derived compatibility

## What works today
  Concise capability/status matrix

## Install
  Validated environment
  Arch/Omarchy installation
  Private-prerelease caveat

## Start using it
  New terminal
  Recent sessions
  Reopen
  Essential pane and tab controls

## Automation
  JSON/NDJSON
  SSH relay
  MCP
  Security boundary
  Links to complete contracts

## How it works
  Small architecture diagram
  Topology vocabulary
  Link to architecture documentation

## Documentation
  Reader-oriented map

## Development
  Short validation commands
  Link to contributor documentation

## Foot lineage
## License
```

The README should become a navigable product entry point, not a condensed copy
of every specialist document. Measure success by comprehension and information
ownership rather than an arbitrary line count.

## Current-capability presentation

Replace the opening milestone paragraph with a reader-oriented table whose
claims are verified against the implementation and retained evidence:

| Area | Intended status language |
| --- | --- |
| Native Wayland terminal | Implemented and validated |
| Persistent sessions and restore | Implemented and validated |
| Panes, Dojos, and Window-local tabs | State the exact accepted Plan 0019 status at rewrite time |
| Multi-client control | Implemented and validated |
| JSON/NDJSON automation | Implemented and validated |
| SSH relay | Implemented and validated |
| MCP adapter | Implemented and validated |
| Sixel, Kitty, and inline iTerm2 images | Supported documented subset |
| Arch/Omarchy packaging | Private prerelease package validated |
| Public distribution | Not released |
| Nix and broader distribution | Planned |

Do not copy this table mechanically. Milestone 1 must verify each row against
current code, plans, package state, and evidence before publication.

## Documentation ownership

### Create `docs/status.md`

Make this the authoritative maturity document. It should contain:

- the current maturity label;
- the difference between implementation maturity and availability;
- validated product areas;
- supported and validated environments;
- open public-release gates;
- known limitations;
- compatibility and support-policy status;
- deferred capabilities; and
- links to retained evidence and the roadmap.

README and package documentation should summarize and link to this document
rather than maintain independent long status paragraphs.

### Create `docs/usage.md`

Move detailed human operation here:

- Lair, Dojo, Splint, and Window concepts;
- new-terminal, sessions, reopen, and explicit Window workflows;
- pane and Window-local tab behavior;
- the complete keyboard and pointer contract;
- selection, clipboard, search, and control transfer;
- restore, reset, and persistence semantics; and
- daily-use examples.

The README retains only the first workflow and essential controls.

### Create `docs/cli.md`

Move the command cookbook here:

- topology inspection;
- creation and mutation;
- restore and lifecycle;
- snapshot and terminal input;
- human versus machine output;
- stable IDs and process incarnation; and
- links to automation schemas and policy.

Security-sensitive machine contract details remain authoritative in
[`automation.md`](../automation.md).

### Expand contributor documentation

Expand the existing `CONTRIBUTING.md` or add `docs/development.md` for:

- isolated test-daemon commands;
- manual daemon/client development;
- workspace validation;
- benchmarks and Foot-oracle workflows;
- fuzzing; and
- graphical-test guardrails.

README should retain only the standard formatter, linter, test command, and a
link.

### Keep specialist documentation authoritative

- [`architecture.md`](../architecture.md) — ownership and system design
- [`automation.md`](../automation.md) — machine contract and policy
- [`mcp.md`](../mcp.md) — MCP adapter
- [`remote.md`](../remote.md) — SSH relay
- [`images.md`](../images.md) — image compatibility and bounds
- [`configuration.md`](../configuration.md) — configuration and Omarchy integration
- [`headless.md`](../headless.md) — service operation
- [`roadmap.md`](../roadmap.md) — completed phases and future work

Move content instead of duplicating it. Each detailed subject should have one
clear authority.

## Public surfaces to update

### `README.md`

Remove or replace:

- “proposed Rust-based evolution”;
- the oversized opening validation callout;
- phase-number narration in product sections;
- protocol-version details in daily-use prose;
- the complete CLI cookbook;
- exhaustive keyboard and behavior descriptions;
- “Try the scaffold” as primary user framing; and
- “Research direction” as a current-product heading.

Rename the final research section to “Design authority and lineage,” retaining
links to ADR 0001 and historical research.

### `docs/roadmap.md`

Replace “This is an early roadmap” with language such as:

> This roadmap records completed implementation phases and the remaining path
> from advanced private prerelease to public distribution.

The roadmap is now both a completion ledger and a forward plan.

### `docs/pre-planning-research.md`

Keep historical content intact and add an archival banner:

> Historical pre-implementation research. Current product and architecture
> status are documented in `README.md`, `docs/status.md`, and
> `docs/architecture.md`.

Historical uses of “proposed” do not need rewriting when their time context is
clear.

### Distribution metadata

Synchronize short descriptions in:

- `packaging/PKGBUILD`;
- `dist/metainfo/com.oldjobobo.splinterm.metainfo.xml`;
- `dist/applications/com.oldjobobo.splinterm.desktop`; and
- Cargo package descriptions where appropriate.

Preferred short description:

> Persistent Wayland terminal for humans and bounded automation

Package-specific status may say “advanced private prerelease,” but desktop and
AppStream summaries should not carry a long maturity explanation.

## Dependency-ordered milestones

### Milestone 0 — preserve the active worktree

Record the documentation baseline and identify pre-existing uncommitted changes.
Do not overwrite or revert active Plan 0019 or client-decomposition work. This
plan authorizes documentation changes only when implementation begins.

### Milestone 1 — build the status truth table

Audit claims across:

- `README.md`;
- architecture, roadmap, packaging, automation, MCP, remote, image, and headless
  documentation;
- current accepted and proposed plans;
- package and desktop metadata; and
- retained validation evidence.

Classify every major claim as implemented, validated, supported, proposed,
deferred, or unreleased. Resolve contradictions before writing marketing copy.

The truth table must specifically determine the accepted state of Window-local
Dojo tabs at rewrite time rather than inheriting a stale claim.

### Milestone 2 — establish status authority

Create `docs/status.md`. Define “advanced private prerelease,” supported
platform scope, validated capabilities, limitations, and remaining public-
release gates. Link to evidence rather than copying detailed logs.

### Milestone 3 — rewrite the README opening and product narrative

Install the product sentence, plain-language explanation, platform sentence,
and maturity callout. Add “Why Splinterm” and the verified capability matrix.
Remove contradictory “proposed” language.

Validate this coherent opening before moving the rest of the README.

### Milestone 4 — extract detailed human usage and CLI reference

Create `docs/usage.md` and `docs/cli.md`. Move detailed commands, bindings,
restore/reset behavior, and advanced interaction semantics out of README. Keep
links and a small first-run workflow in README.

### Milestone 5 — complete README information architecture

Reorder installation, first use, automation, architecture, documentation,
development, lineage, and license sections. Replace implementation chronology
with outcomes and reader paths.

### Milestone 6 — align roadmap and historical framing

Update the roadmap opening and mark pre-planning research as historical. Do not
rewrite old ADR or plan decisions merely to make them sound current.

### Milestone 7 — synchronize public metadata

Align desktop, AppStream, package, and Cargo descriptions with the short product
sentence and maturity taxonomy. Keep field-length and ecosystem conventions in
mind.

### Milestone 8 — product and technical review

Run two independent read-only reviews at the coherent documentation boundary:

1. **Product/readability review**
   - Can a new reader identify the product, differentiator, maturity, and first
     action in thirty seconds?
   - Does the README lead with outcomes rather than implementation history?
   - Is “substrate” explained plainly?

2. **Technical-accuracy review**
   - Does every capability claim match code and retained evidence?
   - Are proposed or incomplete tab features accidentally described as
     accepted?
   - Are security boundaries concise but complete?
   - Are public release and compatibility guarantees avoided?

Actionable in-scope findings should be fixed and non-graphically revalidated.

## Validation

Run documentation and metadata validation:

```bash
git diff --check
desktop-file-validate dist/applications/com.oldjobobo.splinterm.desktop
appstreamcli validate --no-net dist/metainfo/com.oldjobobo.splinterm.metainfo.xml
makepkg --printsrcinfo -p packaging/PKGBUILD >/dev/null
```

Also verify:

- all Markdown links resolve;
- README commands match actual `splinterm --help` and subcommand help;
- every moved subject has one authoritative destination;
- “proposed” describes only genuinely unimplemented work or clearly historical
  text;
- the maturity label is consistent across current public surfaces;
- README does not imply public availability, production stability, or broad
  platform support;
- automation wording does not imply trusted-UI or compositor authority;
- security language states concrete controls rather than absolute safety; and
- package and desktop descriptions remain syntactically valid.

No graphical testing is required or authorized for this documentation plan.

## Review checklist

A reader should be able to answer without reading implementation history:

1. What is Splinterm?
2. Why does daemon-owned persistence matter?
3. Why is bounded automation a first-class differentiator?
4. What is implemented and validated today?
5. Why is the project still a private prerelease?
6. Which environment is currently validated?
7. How can a user install and begin using it?
8. Where are detailed usage, automation, architecture, and evidence documents?

A maintainer should be able to answer:

1. Which document owns current maturity?
2. Which wording is permitted for implemented, validated, supported, proposed,
   and deferred behavior?
3. Which claims require evidence before publication?
4. Which details belong outside the README?

## Completion criteria

This plan is complete only when:

- README no longer calls the existing product “proposed”;
- the product sentence leads all current public positioning;
- “advanced private prerelease” is the consistent maturity label;
- implementation maturity and public availability are clearly distinguished;
- persistence and bounded human/automation access are the primary
  differentiators;
- `docs/status.md` is the authoritative current-status source;
- detailed usage, CLI, and development content have authoritative homes outside
  README;
- roadmap and historical research are framed according to their current roles;
- distribution metadata uses aligned short descriptions;
- links, commands, desktop metadata, AppStream metadata, package metadata, and
  `git diff --check` pass;
- recorded product/readability and technical-accuracy reviews have no unresolved
  blockers; and
- no documentation claim overstates security, compatibility, platform support,
  public availability, or feature acceptance.

The protected central message is:

> **Splinterm is a persistent, security-conscious terminal substrate designed
> equally for humans and bounded automation.**
