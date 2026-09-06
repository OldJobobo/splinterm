import assert from 'node:assert/strict';
import { readFileSync } from 'node:fs';
import { test } from 'node:test';

const html = readFileSync(new URL('../src/pages/index.astro', import.meta.url), 'utf8');
const css = readFileSync(new URL('../src/styles/site.css', import.meta.url), 'utf8');
const glyphs = JSON.parse(readFileSync(new URL('../../assets/fonts/splinter-display/glyphs.json', import.meta.url), 'utf8'));

test('only i and the final m receive wordmark accent styling, with no added spaces', () => {
  assert.match(html, />spl<span class="wordmark-i">i<\/span>nter<span class="wordmark-m">m<\/span><\/h1>/);
  assert.match(css, /--wordmark-yellow: var\(--signal\);/);
  assert.match(css, /\.intro-wordmark \.wordmark-m \{ color: var\(--wordmark-yellow\); \}/);
});

test('the i gradient divides the font gap, not the dot or stem', () => {
  assert.match(css, /linear-gradient\(to bottom, var\(--wordmark-yellow\) \.26em, var\(--ink\) \.26em\)/);
  const yValues = (path) => [...path.matchAll(/[ML]\s+(-?\d+)\s+(-?\d+)/g)].map((m) => Number(m[2]));
  const [stem, dot] = glyphs.i.contours.map(yValues);
  const split = 820 - 260;
  assert.ok(Math.max(...stem) < split && split < Math.min(...dot));
});

test('forced-color mode retains a visible i', () => {
  assert.match(css, /@media \(forced-colors: active\)\s*\{\s*\.intro-wordmark \.wordmark-i \{ background: none; color: CanvasText; \}/);
});
