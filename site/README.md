# Splinterm site

This directory contains the public Astro/Starlight alpha site for Splinterm plus the public coming-soon landing page.

## Surfaces

| Surface | Source/build output | Cloudflare Pages project | Hostname |
| --- | --- | --- | --- |
| Public placeholder | `coming-soon/` | `splinterm-coming-soon` | `splinterm.com`, `www.splinterm.com` |
| Public alpha site and docs | `dist/` from Astro | `splinterm-preview` | `preview.splinterm.com` |

The alpha site, its documentation routes, and the `splinterm-preview.pages.dev` fallback are public. The previous Cloudflare Access email allowlist was removed when the repository entered public alpha.

## Run locally

```bash
cd site
npm install
npm run dev
```

Astro prints the local URL, normally `http://localhost:4321`.

The standalone placeholder can be served from `site/coming-soon/` with any static file server.

## Validate

```bash
npm run validate
npm run preview
```

`npm run validate` type-checks the Astro project, builds every static route and search index, and verifies generated local page and asset links stay inside `dist/`.

The generated `dist/` directory is local build output and is not committed.

## Deploy

Wrangler must be authenticated to the Cloudflare account that owns `splinterm.com`.

```bash
npm run deploy:coming-soon
npm run deploy:preview
```

The first command uploads `coming-soon/` to the public Pages project. The second validates and builds the full public alpha site before uploading `dist/` to its Pages project.
