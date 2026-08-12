# Splinterm site

This directory contains the unified public Astro/Starlight alpha site for Splinterm.

## Surfaces

| Surface | Source/build output | Cloudflare Pages project | Hostname |
| --- | --- | --- | --- |
| Primary public site and docs | `dist/` from Astro | `splinterm-coming-soon` | `splinterm.com`, `www.splinterm.com` |
| Public preview mirror | `dist/` from Astro | `splinterm-preview` | `preview.splinterm.com` |

Both deployments, their documentation routes, and the Pages fallback hostnames are public. The production project retains its historical Cloudflare project name, but it serves the complete site rather than a placeholder. The previous Cloudflare Access email allowlist was removed when the repository entered public alpha.

## Run locally

```bash
cd site
npm install
npm run dev
```

Astro prints the local URL, normally `http://localhost:4321`.

## Validate

```bash
npm run validate
npm run preview
```

`npm run validate` type-checks the Astro project, builds every static route and search index, and verifies generated local page and asset links stay inside `dist/`. The build also emits `sitemap.xml`, `robots.txt`, the SVG favicon, and a `/favicon.ico` compatibility redirect.

The generated `dist/` directory is local build output and is not committed.

## Deploy

Wrangler must be authenticated to the Cloudflare account that owns `splinterm.com`.

For routine releases, validate once and deploy the same build to preview followed by production:

```bash
npm run deploy
```

The individual `deploy:preview` and `deploy:production` commands remain available for recovery or targeted testing. Avoid running several production deploys in quick succession; use the unified command so one validated build advances both Pages projects in a predictable order.

`.github/workflows/site.yml` applies the same sequence on pushes that change `site/`, cancels superseded runs, and verifies the exact preview deployment before promoting the build. Enable it by setting the `CLOUDFLARE_PAGES_DEPLOY` Actions variable to `enabled` and adding `CLOUDFLARE_ACCOUNT_ID` and `CLOUDFLARE_API_TOKEN` Actions secrets with Cloudflare Pages edit access.
