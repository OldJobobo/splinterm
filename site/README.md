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

`npm run validate` type-checks the Astro project, builds every static route and search index, and verifies generated local page and asset links stay inside `dist/`.

The generated `dist/` directory is local build output and is not committed.

## Deploy

Wrangler must be authenticated to the Cloudflare account that owns `splinterm.com`.

```bash
npm run deploy:production
npm run deploy:preview
```

Each command validates and builds the unified public alpha site before uploading `dist/` to the corresponding Pages project.
