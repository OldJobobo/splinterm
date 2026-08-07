# Plan 0026: public website and documentation site

- **Status:** Proposed
- **Date:** 2026-08-07
- **Product/content dependency:** [Plan 0021: product positioning and public documentation](0021-product-positioning-and-public-documentation.md)
- **Technical authority:** current implementation, [Architecture](../architecture.md), and specialist documentation under `docs/`
- **Target origin:** `https://splinterm.com`
- **Hosting target:** Cloudflare Workers with static assets
- **Scope:** product landing page, user-facing documentation, contributor documentation boundary, static deployment, and documentation migration

## Decision

Build one Astro site that serves both the Splinterm product landing page and its documentation:

```text
https://splinterm.com/                  product landing page
https://splinterm.com/docs/             user documentation home
https://splinterm.com/docs/...          task-oriented user guides and reference
https://splinterm.com/docs/development/ contributor and maintainer documentation
```

Use Starlight for documentation structure, navigation, local static search, table of contents, and Markdown/MDX content. Implement the root landing page as a custom Astro page so it can establish a distinct product identity without fighting a documentation template. Share typography, color tokens, navigation, footer, metadata, and reusable components across both surfaces.

Generate a fully static site and deploy its output to Cloudflare Workers static assets. Do not introduce server-side rendering, a database, authentication, or a Worker script until a concrete requirement needs runtime behavior. Configure an explicit custom 404 page. Use Wrangler and a checked-in configuration so local, CI, preview, and production deployments share one reproducible path.

This plan does not replace Plan 0021. Plan 0021 owns product positioning, maturity language, README decomposition, and the new authoritative content documents. This plan owns how that content becomes a coherent public website.

## Why one site

A single site avoids the most likely early failure mode: a polished marketing page and a separate generic documentation install that drift in language, design, navigation, and deployment.

One Astro project provides:

- one content source and link graph;
- one visual system;
- one build and preview command;
- one Cloudflare deployment;
- one canonical origin for search engines and shared links;
- freedom to make `/` expressive while keeping `/docs/` calm and task-oriented; and
- a clean path to add release notes, downloads, or dynamic Worker behavior later without requiring it now.

## Audience model

The site should make three audiences explicit without presenting three separate products.

### Evaluators

They need to understand within one minute:

1. what Splinterm is;
2. why daemon-owned persistence matters;
3. how humans and bounded automation share one terminal topology;
4. which environment is currently validated; and
5. why the project is still an advanced private prerelease.

Primary surface: landing page and current-status page.

### Users and operators

They need task-oriented paths for installation, first launch, sessions, panes, tabs, configuration, persistence, headless operation, troubleshooting, and supported integrations.

Primary surface: `/docs/`.

### Contributors and integration authors

They need architecture, source layout, development setup, validation, ADRs, plans, public automation contracts, schemas, packaging, benchmarks, and retained evidence.

Primary surface: `/docs/development/` and explicitly labeled reference sections. These pages remain discoverable but do not crowd the user journey.

## Information architecture

### Global navigation

Keep the top navigation small and stable:

- **Product** → `/`
- **Docs** → `/docs/`
- **Status** → `/docs/status/`
- **Development** → `/docs/development/`
- **Source** → repository URL when public access is appropriate

Do not add top-level links for every specialist subject. Documentation navigation and contextual links should carry that depth.

### Landing page route: `/`

The landing page has one job: explain the product accurately and send the visitor to the right next step.

Recommended section order:

1. **Hero thesis**
   - Product sentence from Plan 0021.
   - Plain-language persistence explanation.
   - Primary action: **Read the docs**.
   - Secondary action while private: **See what works today**.
   - Replace or add **Install Splinterm** only when public distribution is genuinely available.

2. **Persistent by architecture**
   - A small interactive or animated topology illustration: Window views connect to daemon-owned Lairs, Dojos, and Splints.
   - Show that closing a Window detaches a view rather than killing the session.

3. **One substrate, two kinds of control**
   - Human path: native Wayland windows, tabs, panes, search, clipboard, and explicit control transfer.
   - Automation path: bounded JSON/NDJSON, SSH relay, and MCP under executable identity, scopes, limits, and revocation.
   - Never imply that automation receives trusted graphical authority.

4. **What works today**
   - Concise capability grid sourced from `docs/status.md` rather than maintained independently.
   - Make private prerelease and validated environment conspicuous.

5. **Designed first for Omarchy**
   - Omarchy/Arch and Foot lineage.
   - Theme integration and native Wayland focus.
   - Do not imply broad compositor or distribution support.

6. **Begin with the first workflow**
   - Install or evaluation prerequisites.
   - New terminal → create work → detach → reopen.
   - Link to the complete getting-started guide.

7. **Final documentation gateway**
   - Start using Splinterm.
   - Configure and operate it.
   - Integrate automation.
   - Understand or contribute to it.

The landing page should not contain a protocol inventory, exhaustive keyboard map, implementation chronology, or retained validation logs.

### Documentation route: `/docs/`

Recommended initial sidebar:

```text
Start here
  Overview
  Current status
  Installation
  Quickstart
  Concepts

Use Splinterm
  Sessions and persistence
  Panes and layouts
  Tabs and windows
  Search, selection, and clipboard
  Keyboard and pointer controls
  Restore and reset
  Terminal images

Configure
  Configuration reference
  Omarchy integration
  Foot migration

Operate
  Headless service
  Remote access over SSH
  Security and authorization
  Troubleshooting

Automate and integrate
  Automation overview
  CLI reference
  JSON/NDJSON contracts
  Client integration guide
  MCP adapter

Development
  Contributor guide
  Development setup
  Architecture
  Crate map
  Testing and validation
  Packaging and releases
  ADR index
  Plans and research archive
  Benchmarks and evidence

Project
  Roadmap
  Glossary
  Foot lineage and licenses
```

This is a target navigation model, not a requirement to write every page before launch. Empty categories should not ship.

## Content ownership and migration

### Publish as user-facing documentation after editing

| Current source | Public destination | Required treatment |
| --- | --- | --- |
| `README.md` daily-use material | Quickstart and focused usage guides | Split by user task; remove phase/protocol narration |
| `GLOSSARY.md` | `/docs/glossary/` plus contextual definitions | Keep canonical definitions; add plain-language links |
| `docs/configuration.md` | Configuration, Omarchy integration, Foot migration | Split long page into scannable tasks and reference |
| `docs/images.md` | Terminal image compatibility | Preserve exact supported subset and limits |
| `docs/headless.md` | Headless operation | Keep operator warnings and secure defaults |
| `docs/remote.md` | SSH remote access | Keep exact transport and authority boundary |
| `docs/mcp.md` | MCP adapter | Keep separate installation, policy, and host setup |
| `docs/integrations.md` | Integration author guide | Label as advanced/integration-author content |
| `docs/automation.md` | Machine contract reference | Keep authoritative; add a simpler automation overview before it |
| `docs/packaging.md` | Installation/release internals | Split user install from maintainer packaging details |
| `docs/roadmap.md` | Project roadmap | Publish after current/historical framing from Plan 0021 |

### Create through Plan 0021 before or during migration

- `docs/status.md` — authoritative maturity and availability.
- `docs/usage.md` — authoritative detailed human workflow.
- `docs/cli.md` — authoritative human CLI cookbook and command reference.
- expanded `CONTRIBUTING.md` or `docs/development.md` — contributor entry point.

The site should render or migrate these authorities rather than invent parallel copies.

### Keep out of primary user navigation

- `docs/adr/**` — indexed under Development.
- `docs/plans/**` — indexed as proposed/completed historical plans.
- `docs/spikes/**` — development archive only.
- `docs/benchmarks/**` — evidence archive only.
- `docs/**/artifacts/**` — retained evidence, not ordinary documentation pages.
- `docs/pre-planning-research.md` and `docs/herdr-integration-research.md` — historical/research archive with clear banners.

Artifact trees should not be imported wholesale into Starlight's content collection. Link to repository evidence when needed. This prevents more than sixty artifact Markdown files—including very large review logs—from polluting search, navigation, build time, and the user-facing information model.

## Content source layout

Create the website in a top-level `site/` directory so the Rust workspace and its build stay independent:

```text
site/
├── astro.config.mjs
├── package.json
├── wrangler.jsonc
├── public/
│   ├── favicon.svg
│   ├── og/
│   └── images/
├── src/
│   ├── components/
│   ├── content/
│   │   └── docs/
│   ├── layouts/
│   ├── pages/
│   │   └── index.astro
│   └── styles/
└── tsconfig.json
```

### Initial content strategy

Prefer an explicit migration into `site/src/content/docs/` over automatically mounting the complete repository `docs/` tree.

Reasons:

- current Markdown mixes users, maintainers, plans, spikes, and evidence;
- public pages need titles, descriptions, navigation order, status labels, and redirects;
- some long documents need to be split rather than merely restyled;
- plans and evidence should not enter default search results; and
- explicit migration makes every published claim a deliberate review boundary.

During migration, each published page should name its source authority in frontmatter or a maintainer-only mapping file so drift can be audited. Avoid keeping two independently edited copies long-term. Once the public page is accepted, either move authority to the site content or generate the repository-facing copy from that authority; decide this per content class before broad migration.

## Design direction

The site should feel native to Splinterm rather than like a themed documentation starter.

### Visual thesis

Present the interface as a persistent topology viewed through temporary frames. The memorable element is a restrained topology line that moves from one daemon-owned session graph into multiple terminal views; it should communicate architecture, not imitate decorative terminal rain.

### Principles

- Use the active Splinterm/Omarchy visual language as source material, but choose a stable site palette rather than changing the website with every local theme.
- Let terminal geometry influence grids, code blocks, diagrams, and dividers without making all body text monospace.
- Use a readable text face for prose and a deliberate monospace face for commands, topology labels, and technical metadata.
- Keep docs quieter than the landing page while preserving shared tokens and navigation.
- Treat screenshots as product evidence, not wallpaper. Capture them only through a separately approved graphical sequence.
- Support mobile layouts, visible focus, reduced motion, high contrast, semantic landmarks, and useful no-JavaScript rendering.

### Content design guardrails

- Explain Splinterm's coined nouns only when they become necessary.
- Lead with user outcomes, then reveal architecture.
- Use “security-conscious” and name concrete controls; do not claim absolute security.
- Use “implemented,” “validated,” “supported,” “proposed,” and “deferred” according to Plan 0021.
- Never turn terminal output into an instruction, approval, or authority signal in examples or interactive demos.

## Technical baseline

### Site framework

- Astro with static output.
- Starlight for documentation pages.
- TypeScript in strict mode.
- Markdown/MDX content collections with schema-validated frontmatter.
- Starlight's static Pagefind search for public user and reference pages.
- No client framework unless a specific island requires it.

### Cloudflare deployment

Use Workers static assets rather than beginning a new Pages project. Cloudflare's current Workers model deploys static assets and optional Worker logic as one unit, supports custom domains, and keeps a future dynamic path open without requiring one now.

Proposed production configuration characteristics:

- assets directory points to Astro's `dist/` output;
- explicit `not_found_handling: "404-page"`;
- `splinterm.com` attached as a Workers custom domain after the domain is an active Cloudflare zone;
- a `workers.dev` or preview URL used for non-production review;
- no runtime asset binding or Worker `main` entry for the static-only first milestone;
- deployment through a pinned Wrangler development dependency; and
- production deployment gated on successful build, link, and content checks.

Do not deploy or change DNS under this plan without explicit user approval.

### Search, metadata, and machine readability

The first release should include:

- local static documentation search;
- canonical URLs;
- sitemap and `robots.txt`;
- Open Graph and social-card metadata;
- structured data for the software application and documentation where accurate;
- RSS only when a changelog or release-notes stream exists;
- a generated `llms.txt`/Markdown-oriented index only after its content and security implications are reviewed; and
- no analytics beacon by default until the desired measurement and privacy policy are explicitly chosen.

### Redirects

Create redirects whenever an accepted repository document acquires a stable website route or a website page is split/renamed. Keep routes task-oriented and omit `.html`. A route manifest should make redirects reviewable in CI.

## Delivery milestones

### Milestone 0 — inventory and decisions

- Record the current dirty worktree and avoid overlapping active changes.
- Complete the status truth table required by Plan 0021.
- Confirm repository visibility and the source link policy for a private prerelease.
- Confirm whether `splinterm.com` is already an active Cloudflare zone.
- Decide whether preview deployments should be public, unlisted, or protected by Cloudflare Access.
- Decide whether docs content becomes authoritative under `site/` or is generated from selected root `docs/` files.

### Milestone 1 — static site foundation

- Create `site/` with Astro, Starlight, strict TypeScript, and pinned package tooling.
- Add global tokens, shared header/footer, basic metadata, sitemap, robots, and custom 404.
- Add Wrangler static-assets configuration without deploying.
- Add local `dev`, `build`, `preview`, `check`, and link-check commands.

Acceptance: a static build contains `/`, `/docs/`, and `/404.html`; no Worker runtime is required.

### Milestone 2 — landing-page first slice

- Implement the product hero and plain-language architecture.
- Implement the topology signature element with reduced-motion and no-JavaScript behavior.
- Add differentiators, current-status summary, validated platform scope, and documentation gateway.
- Use real product copy from Plan 0021; do not use placeholder marketing claims.
- Use code-native diagrams or already approved assets; graphical capture requires separate approval.

Acceptance: a new visitor can identify product, differentiator, maturity, and next action in under one minute.

### Milestone 3 — user documentation first slice

Publish the smallest complete user journey:

1. overview;
2. current status;
3. installation/evaluation prerequisites;
4. quickstart;
5. concepts;
6. sessions and persistence;
7. configuration; and
8. troubleshooting.

Add the glossary and first redirects. Keep incomplete categories out of navigation.

Acceptance: a user can go from environment check to new terminal, detach, and reopen without reading README implementation history.

### Milestone 4 — advanced operation and automation

- Publish headless, SSH relay, security/authorization, image compatibility, and MCP guides.
- Add a simple automation overview before the complete JSON/NDJSON contract.
- Publish CLI and integration-author references.
- Ensure every automation page distinguishes topology from native Window control and terminal data from authority.

### Milestone 5 — development boundary

- Publish contributor entry, development setup, architecture, crate map, testing, packaging, and release guidance.
- Generate curated indexes for ADRs, plans, spikes, benchmarks, and evidence.
- Exclude archival/evidence pages from default user search unless there is a demonstrated reason to include them.

### Milestone 6 — quality and accessibility

- Validate internal and external links.
- Validate frontmatter and route uniqueness.
- Test keyboard navigation and visible focus.
- Test representative mobile and desktop layouts.
- Test reduced motion, contrast, headings, landmarks, code overflow, and search.
- Audit bundle size and remove nonessential client JavaScript.
- Verify canonical, sitemap, robots, 404, and social metadata.

Graphical browser testing requires the bounded approval described in repository guardrails before it begins.

### Milestone 7 — Cloudflare preview

- Build a clean static artifact.
- Create the Workers project and deploy a non-production preview only after approval.
- Verify asset routing, cache behavior, custom 404, headers, preview visibility, and rollback.
- Keep `splinterm.com` unchanged during preview validation.

### Milestone 8 — production launch

- Obtain explicit approval for DNS/custom-domain and production deployment changes.
- Attach `splinterm.com` as the Workers custom domain.
- Verify TLS, canonical redirects, sitemap, robots, social cards, and production links.
- Record rollback and redeployment commands.
- Announce public installation only if distribution status has independently changed.

## Initial page backlog

### Launch-blocking

- `/`
- `/docs/`
- `/docs/status/`
- `/docs/install/`
- `/docs/quickstart/`
- `/docs/concepts/`
- `/docs/sessions/`
- `/docs/configuration/`
- `/docs/troubleshooting/`
- `/docs/development/`
- `/404.html`

### Next user/operator slice

- panes and layouts;
- tabs and windows;
- controls;
- search/selection/clipboard;
- restore/reset;
- terminal images;
- headless operation;
- remote access; and
- security/authorization.

### Next integration/development slice

- automation overview;
- CLI reference;
- JSON/NDJSON contract;
- MCP setup;
- integration author guide;
- architecture and crate map;
- testing and validation;
- packaging/releases;
- roadmap and glossary; and
- curated decision/evidence indexes.

## Validation

Before a preview deployment:

```bash
# From site/
npm run check
npm run build
npm run test:links

# From repository root
git diff --check
```

The implementation milestone should select and pin the exact package manager and commands. CI should additionally verify:

- static output contains the required routes;
- no broken internal links or duplicate anchors;
- no draft page enters production navigation or search;
- archived plans, spikes, and artifact logs do not enter ordinary user search;
- canonical URLs use `https://splinterm.com`;
- the custom 404 is emitted and configured;
- code examples match current CLI help;
- status language matches `docs/status.md`;
- public pages do not imply production stability, public package availability, broad platform support, trusted-UI authority for automation, or native Window control through topology APIs; and
- dependency and generated-output changes are reviewable and reproducible.

Cloudflare preview validation should use Wrangler's deployment output and a bounded HTTP smoke suite for:

- `/`;
- `/docs/`;
- one nested docs route;
- one static asset;
- one redirected route; and
- one unknown route returning the custom 404.

## Decisions needed before implementation

1. Is the repository itself public when the website launches, and should **Source** appear in global navigation?
2. Is `splinterm.com` already active in the intended Cloudflare account/zone?
3. Should preview URLs be public, unlisted, or protected with Cloudflare Access?
4. Should the website content under `site/` become authoritative, or should selected root documentation generate into it?
5. Is the first public call to action **Read the docs**, **Join the prerelease**, or another honest availability action?
6. Which real screenshots or recordings already exist and are approved for public use?
7. Should privacy-preserving analytics be enabled, and what decision will that data support?

Only questions 1, 4, and 5 materially block the first content scaffold. Cloudflare account and DNS decisions block deployment, not local implementation.

## Completion criteria

This plan is complete when:

- `splinterm.com` serves one coherent product and documentation site;
- `/` clearly explains Splinterm, its persistence model, bounded automation, validated platform scope, and private-prerelease status;
- `/docs/` provides a complete first user journey;
- development and archival material is available but visibly separated from user guidance;
- every major subject has one authoritative source;
- plans, spikes, benchmarks, and evidence do not pollute primary navigation or search;
- the static build, links, accessibility checks, metadata checks, and Cloudflare HTTP smoke suite pass;
- production DNS/deployment changes have explicit approval and a recorded rollback;
- current public pages do not overstate security, compatibility, release availability, or platform support; and
- README, website, package metadata, and current-status documentation use the same product and maturity language.
