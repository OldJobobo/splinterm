// Standalone behavior checks; no npm dependencies or browser session required.
const assert = require('node:assert/strict');
const { readFileSync, existsSync } = require('node:fs');
const path = require('node:path');
const { test } = require('node:test');
const vm = require('node:vm');

const html = readFileSync(path.join(__dirname, 'viewer.html'), 'utf8');
const source = html.match(/<script>([\s\S]*?)<\/script>/)[1];

function viewer({ mobile = false, fontResult = 'loaded' } = {}) {
  const elements = {};
  for (const id of ['sample', 'size', 'size-value', 'coverage', 'font-status', 'reset']) {
    elements[id] = {
      value: id === 'sample' ? 'splinterm' : '', textContent: '', listeners: {},
      addEventListener(event, callback) { this.listeners[event] = callback; },
      focus() { this.focused = true; },
      classList: { toggle(name, value) { this[name] = value; } },
    };
  }
  const properties = {};
  const context = vm.createContext({
    document: {
      getElementById(id) { return elements[id]; },
      documentElement: { style: { setProperty(key, value) { properties[key] = value; } } },
      fonts: { load() {
        if (fontResult === 'error') return Promise.reject(new Error('blocked'));
        return Promise.resolve(fontResult === 'loaded' ? [{}] : []);
      } },
    },
    window: { matchMedia() { return { matches: mobile }; } },
  });
  vm.runInContext(source, context, { timeout: 1000 });
  return { elements, properties };
}

for (const [mobile, size] of [[false, '128'], [true, '64']]) {
  test(`initial size and range changes (${mobile ? 'mobile' : 'desktop'})`, () => {
    const { elements, properties } = viewer({ mobile });
    assert.equal(elements.size.value, size);
    assert.equal(properties['--sample-size'], `${size}px`);
    elements.size.value = '200';
    elements.size.listeners.input();
    assert.equal(properties['--sample-size'], '200px');
    assert.equal(elements['size-value'].value, '200 px');
  });
}

test('coverage matches all 104 mapped characters and warns for unsupported input', () => {
  const { elements } = viewer();
  elements.sample.value = Array.from({ length: 95 }, (_, i) => String.fromCodePoint(32 + i)).join('')
    + '\u00a0–—‘’“”•…\n\t';
  elements.sample.listeners.input();
  assert.equal(elements.coverage.classList.warning, false);
  elements.sample.value = 'café é 🐀';
  elements.sample.listeners.input();
  assert.equal(elements.coverage.classList.warning, true);
  assert.match(elements.coverage.textContent, /Not in this font: é 🐀\./);
  elements.reset.listeners.click();
  assert.equal(elements.sample.value, 'splinterm');
  assert.equal(elements.sample.focused, true);
  assert.equal(elements.coverage.classList.warning, false);
});

for (const [fontResult, expected] of [
  ['loaded', /Local WOFF2 loaded/], ['empty', /Font did not load/], ['error', /browser blocks local fonts/],
]) {
  test(`font loading state: ${fontResult}`, async () => {
    const { elements } = viewer({ fontResult });
    await new Promise(resolve => setImmediate(resolve));
    assert.match(elements['font-status'].textContent, expected);
  });
}

test('local download and font resource paths exist', () => {
  const targets = [...html.matchAll(/href="([^"]+)"/g)].map(match => match[1]);
  targets.push(html.match(/src: url\("([^"]+)"\)/)[1]);
  for (const target of targets) {
    assert.ok(existsSync(path.join(__dirname, target)), target);
    assert.ok(!target.includes('://'), 'viewer must remain local-only');
  }
});
