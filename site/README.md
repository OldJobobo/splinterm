# Splinterm site

This directory contains the unified public Astro/Starlight product site for Splinterm.

## Surfaces

| Surface | Source/build output | Cloudflare Pages project | Hostname |
| --- | --- | --- | --- |
| Primary public site and docs | `dist/` from Astro | `splinterm-coming-soon` | `splinterm.com`, `www.splinterm.com` |
| Public preview mirror | `dist/` from Astro | `splinterm-preview` | `preview.splinterm.com` |

Both deployments, their documentation routes, and the Pages fallback hostnames are public. The production project retains its historical Cloudflare project name, but it serves the complete site rather than a placeholder. The previous Cloudflare Access email allowlist was removed when the repository entered public alpha.

## Run locally

```bash
cd site
npm ci
npm run dev
```

Astro prints the local URL, normally `http://localhost:4321`.

## Validate

```bash
npm run validate
npm run preview
```

`npm run validate` runs the release, typography, and font-integration regression tests, type-checks the Astro project, builds every static route and search index, verifies generated local page and asset links stay inside `dist/`, and checks current-release copy, critical routes, and the bundled wordmark font. `npm test` runs the in-memory regression suite independently. The build also emits `sitemap.xml`, `robots.txt`, the SVG favicon, and a `/favicon.ico` compatibility redirect.

The generated `dist/` directory is local build output and is not committed.

## Wordmark typography

The homepage's main “splinterm” title uses **Splinter Display Heavy v0.2**, the
original font in [`../assets/fonts/splinter-display/`](../assets/fonts/splinter-display/).
CSS and the homepage-only preload import its canonical WOFF2 directly; Vite emits
one content-hashed asset in `dist/_astro/`. There is no manually synchronized copy
in `public/`, no external font service, and no font-build dependency in the site
build. Build from the repository checkout, not a standalone copy of `site/`.

Only `.intro-wordmark` uses the display font. Body text, navigation, code, roadmap
headings, and documentation typography are unchanged. Weight 900 and zero extra
letter spacing preserve the font's intended spacing and kerning; `font-display: swap`
keeps the title visible while loading.

`npm run check:font` checks the built font against the canonical bytes, confirms
that CSS and the CORS-enabled preload use the same URL, and guards title styling
and homepage-only preloading. `npm run test:font` exercises failure cases without
building the site; these tests also run under `npm test`. Both site workflows watch
canonical WOFF2 changes.

## Voice and copy

Playful names, straightforward explanations. Keep Lair, Dojo, and Splint; let the name and artwork carry the personality without announcing the joke.

- Lead with what a person can do: open, split, return, configure, connect.
- Explain a term when it first becomes useful. A Lair is a workspace, a Dojo is a layout, and a Splint is a terminal pane.
- Prefer concrete examples to phrases such as “shared substrate” or “closed authority.” Keep precise protocol terms in the reference docs where they help.
- Say important limits plainly. “Restarting the background service ends running commands” is more useful than “no live daemon-upgrade handoff.” Keep the exact technical meaning behind the shorter wording.
- Do not trade accuracy for friendliness: qualify default persistence, explain permission requirements, and separate planned work from shipped features.
- Remove repeated explanations before adding another section. A heading should help readers find something, not make them solve a metaphor.
- Keep reading text at 18–20px and supporting labels at least 16px at the default root size, expressed in `rem`. Use normal weights, readable contrast, and reflow narrow layouts rather than shrinking text. The typography tests guard key source declarations and solid-surface contrast; also check computed styles, overflow, and clipping in the local preview at mobile and desktop widths.

## Release content ownership

The site describes the **shipped release**, not everything implemented on `main`. For 0.1 releases, compare the published GitHub release, AUR package versions, exact release-tag source, and the post-publication record on `maint/0.1` before editing. Do not merge an entire maintenance branch merely to synchronize documentation.

`src/data/release.json` owns the repeated Astro version/channel labels, package version, release URL, and exact source/publication record commits. Both the custom layout and Starlight emit a `splinterm-release` meta marker. The status guide retains readable, concrete release prose; `check:release` verifies it against the shared metadata. Specialist guides must describe shipped behavior, not unreleased keybindings or other development features.

For each release:

1. Verify the actual published release and package identities; metadata is an audited snapshot, not a live GitHub/AUR lookup.
2. Update the shared metadata, status prose and evidence links together. Reconcile installation, upgrades, configuration, lifetime rules, quickstart, and both roadmaps.
3. Review capability claims against the exact shipped source. Preserve narrow platform and compatibility boundaries.
4. Run `npm run validate` and inspect the actual diff. Obtain independent review before integration.
5. Publish only after approval and verify the deployed release markers and critical routes.

`check:release` checks every generated page for its release marker, stale current alpha/beta/RC and edge-installer claims, and noncurrent release links. It also checks required routes in `sitemap.xml`, visible version/release links, and the status package identity. Deliberate historical release copy may live inside a small HTML `<aside data-release-history>…</aside>`; that exclusion does not apply to the rest of a page or its description. The checks cover known drift patterns, not arbitrary product semantics or external URL availability. They currently require a stable SemVer baseline; changing release-channel policy requires reviewing the checks too.

Do not restore numerical performance comparisons without a public methodology reference identifying hardware, exact builds, daemon/prestart conditions, measurement scope, and limitations. The former landing figures were removed because the site did not provide that evidence.

`.github/workflows/site-checks.yml` validates website pull requests and relevant `main` pushes without deployment credentials, independently of the deployment switch.

## Deploy

Wrangler must be authenticated to the Cloudflare account that owns `splinterm.com`.

Deployment, enabling automatic publishing, and changing hosting credentials require separate approval. Content changes do not enable deployment.

For a staged, approved publication, validate once and upload that build to preview:

```bash
npm run validate
npm run deploy:preview:upload -- --commit-hash REVIEWED_WEBSITE_COMMIT
```

Replace `REVIEWED_WEBSITE_COMMIT` with the full reviewed website commit hash. Record the exact preview deployment URL and verify its source commit, release marker, landing, status, install, packaging, roadmap, and sitemap routes. Check that routes return their intended content, not merely an HTTP 200 fallback. Visual/browser testing is a separate approved validation step; non-graphical checks do not establish layout quality.

After the preview is accepted and production publication is approved, promote the **same unchanged `dist/`**, without rebuilding:

```bash
npm run deploy:production:upload -- --commit-hash REVIEWED_WEBSITE_COMMIT
```

Repeat the route and release-marker checks against the exact production deployment and public hostname. Record the source commit, both deployment IDs/URLs, and the previous production deployment for rollback. Do not publish from a dirty checkout while attributing it to an unchanged commit.

`npm run deploy` remains the automatic preview-then-production path for an explicitly approved uninterrupted sequence. The individual `deploy:preview` and `deploy:production` commands rebuild and are not the same-build manual promotion workflow above.

`.github/workflows/site.yml` applies its automatic sequence on relevant **`main`** pushes or manual dispatch, cancels superseded runs, and checks the exact deployment commit and sitemap. It does not automatically incorporate `maint/0.1` release documentation or establish that release copy is current. Only after explicit approval, enable it by setting the `CLOUDFLARE_PAGES_DEPLOY` Actions variable to `enabled` and adding `CLOUDFLARE_ACCOUNT_ID` and `CLOUDFLARE_API_TOKEN` Actions secrets with Cloudflare Pages edit access.
