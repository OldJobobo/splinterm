# Splinterm local site

This directory contains the local Astro and Starlight prototype for the Splinterm landing page and public documentation.

## Run locally

```bash
cd site
npm install
npm run dev
```

Astro prints the local URL, normally `http://localhost:4321`.

## Validate a static build

```bash
npm run validate
npm run preview
```

`npm run validate` type-checks the Astro project, builds every static route and search index, and verifies generated local page and asset links stay inside `dist/`.

The generated `dist/` directory is local build output and is not committed.

## Scope

This milestone is intentionally local-only. It contains no Wrangler configuration, Cloudflare project identity, deployment token, custom-domain route, analytics beacon, or DNS automation.
