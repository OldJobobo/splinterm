import assert from 'node:assert/strict';
import { readFileSync } from 'node:fs';
import test from 'node:test';

const site = readFileSync(new URL('../src/styles/site.css', import.meta.url), 'utf8');
const docs = readFileSync(new URL('../src/styles/starlight.css', import.meta.url), 'utf8');

// Bounded source guards, not a substitute for computed-style and responsive checks.
function rules(css, selector) {
  return [...css.matchAll(/([^{}]+)\{([^{}]*)\}/g)]
    .filter(([, selectors]) => selectors.trim().split(',').map(s => s.trim()).includes(selector))
    .map(([, , declarations]) => declarations);
}

function variables(css, selector = ':root') {
  return Object.fromEntries([...rules(css, selector).join(';').matchAll(/(--[\w-]+):\s*([^;]+);/g)]
    .map(([, name, value]) => [name, value.trim()]));
}

function assertSizeFloor(css, selectors, floor) {
  const vars = variables(css);
  for (const selector of selectors) {
    const declarations = rules(css, selector).join(';')
      .replace(/var\((--[\w-]+)\)/g, (original, name) => vars[name] ?? original);
    const fonts = [...declarations.matchAll(/(?:^|;)\s*font(?:-size)?:\s*([^;]+)/g)];
    assert.ok(fonts.length, `Missing font declaration: ${selector}`);
    for (const [, font] of fonts) {
      const size = font.match(/([\d.]+)rem/);
      assert.ok(size, `Expected rem-based type: ${selector}: ${font}`);
      assert.ok(Number(size[1]) >= floor, `${selector}: ${font} is below ${floor}rem`);
      assert.doesNotMatch(font, /^300\s/, `Thin reading text: ${selector}`);
    }
  }
}

test('reading text stays at least 18px at the default root size', () => {
  assertSizeFloor(site, [
    'body', '.hero-note', '.topology-scene figcaption', '.sequence small',
    '.wayland-benefits p', '.wayland-boundary', '.control-card li', '.boundary-note',
    '.capability-row strong', '.public-horizon-outcomes li', '.public-horizon-signal',
    '.roadmap-boundaries li', '.gateway-links small', '.site-footer p',
  ], 1.125);
  assertSizeFloor(docs, ['.docs-home-lede', '.route-copy small', '.docs-home-footer p'], 1.125);
});

test('controls and supporting labels stay at least 16px', () => {
  assertSizeFloor(site, [
    '.site-header nav a', '.intro-status', '.intro-install', '.eyebrow', '.button',
    '.topology-toolbar code', '.node-kicker', '.lair-node span', '.window > span',
    '.section-index', '.card-label', '.text-link', '.capability-row i',
    '.command-stack small', '.roadmap-compass ol', '.footer-links',
  ], 1);
  assertSizeFloor(docs, ['.sl-link-button', '.docs-home-kicker', '.route-meta', '.docs-home-footer a'], 1);
  const vars = variables(docs);
  for (const name of ['--sl-text-2xs', '--sl-text-xs', '--sl-text-sm', '--sl-text-body-sm', '--sl-text-code-sm']) {
    assert.ok(parseFloat(vars[name]) >= 1, name);
  }
});

function luminance(hex) {
  const channels = hex.slice(1).match(/../g).map(value => parseInt(value, 16) / 255)
    .map(value => value <= .04045 ? value / 12.92 : ((value + .055) / 1.055) ** 2.4);
  return channels[0] * .2126 + channels[1] * .7152 + channels[2] * .0722;
}

function contrast(a, b) {
  const values = [luminance(a), luminance(b)].sort((x, y) => y - x);
  return (values[0] + .05) / (values[1] + .05);
}

test('secondary text colors meet 4.5:1 on their solid reading surfaces', () => {
  const vars = variables(site);
  for (const background of ['--ground', '--panel', '--panel-raised']) {
    assert.ok(contrast(vars['--dim'], vars[background]) >= 4.5, background);
  }
  for (const theme of [':root', ":root[data-theme='light']"]) {
    const colors = { ...variables(docs), ...variables(docs, theme) };
    for (const background of ['--sl-color-black', '--sl-color-gray-6']) {
      assert.ok(contrast(colors['--sl-color-gray-3'], colors[background]) >= 4.5, `${theme}: ${background}`);
    }
  }
});
