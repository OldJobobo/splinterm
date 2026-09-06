import assert from 'node:assert/strict';
import { mkdirSync, mkdtempSync, readFileSync, rmSync, writeFileSync } from 'node:fs';
import { join, resolve } from 'node:path';
import { test } from 'node:test';
import { checkFont } from './check-font.mjs';

function fixture(t) {
  // Temporary fixtures stay inside the repository, including under CI.
  const temp = mkdtempSync(join(resolve('.'), '.font-check-'));
  t.after(() => rmSync(temp, { recursive: true, force: true }));
  const root = join(temp, 'dist');
  for (const directory of ['_astro', 'roadmap', 'docs']) mkdirSync(join(root, directory), { recursive: true });
  const source = join(temp, 'source.woff2');
  const url = '/_astro/font.hash.woff2';
  writeFileSync(source, 'canonical-font-fixture');
  writeFileSync(join(root, url), 'canonical-font-fixture');
  const preload = `<link rel="preload" href="${url}" as="font" type="font/woff2" crossorigin="anonymous">`;
  writeFileSync(join(root, 'index.html'), `${preload}<link rel="stylesheet" href="/_astro/site.css"><h1 id="intro-title">splinterm</h1>`);
  writeFileSync(join(root, '_astro/site.css'), `
    @font-face{font-family:Splinter Display;src:url(${url})format("woff2");font-weight:900;font-display:swap}
    :root{--display:Splinter Display,var(--sans)}
    .intro-wordmark{font-family:var(--display);font-weight:900;font-synthesis:none;font-kerning:normal;letter-spacing:0}
  `);
  for (const page of ['roadmap', 'docs']) writeFileSync(join(root, page, 'index.html'), '<html></html>');
  function replace(file, before, after) {
    const target = join(root, file);
    writeFileSync(target, readFileSync(target, 'utf8').replace(before, after));
  }
  return { root, source, url, preload, replace };
}

test('accepts matching bundled font and scoped preload', (t) => {
  const f = fixture(t);
  assert.equal(checkFont(f), f.url);
});

test('rejects a stale or substituted font binary', (t) => {
  const f = fixture(t);
  writeFileSync(join(f.root, f.url), 'different-font');
  assert.throws(() => checkFont(f), /differs from the canonical/);
});

test('rejects a different font URL in CSS', (t) => {
  const f = fixture(t);
  f.replace('_astro/site.css', f.url, '/_astro/other.woff2');
  assert.throws(() => checkFont(f), /same font URL/);
});

test('rejects the former negative letter spacing', (t) => {
  const f = fixture(t);
  f.replace('_astro/site.css', 'letter-spacing:0', 'letter-spacing:-.08em');
  assert.throws(() => checkFont(f));
});

test('requires anonymous CORS mode for font preload reuse', (t) => {
  const f = fixture(t);
  f.replace('index.html', 'crossorigin="anonymous"', '');
  assert.throws(() => checkFont(f));
});

test('rejects needless font preloading on non-wordmark routes', (t) => {
  const f = fixture(t);
  writeFileSync(join(f.root, 'docs/index.html'), f.preload);
  assert.throws(() => checkFont(f), /needlessly preloads/);
});

test('rejects external asset URLs', (t) => {
  const f = fixture(t);
  f.replace('index.html', f.url, 'https://example.com/font.woff2');
  assert.throws(() => checkFont(f), /local asset URL/);
});

test('rejects asset paths escaping dist', (t) => {
  const f = fixture(t);
  f.replace('index.html', f.url, '/../source.woff2');
  assert.throws(() => checkFont(f), /escapes dist/);
});

test('requires an actual homepage font-face rule', (t) => {
  const f = fixture(t);
  f.replace('_astro/site.css', '@font-face', '@not-a-font');
  assert.throws(() => checkFont(f), /@font-face is missing/);
});
