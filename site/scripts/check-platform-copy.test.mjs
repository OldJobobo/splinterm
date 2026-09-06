import assert from 'node:assert/strict';
import { readFileSync } from 'node:fs';
import { test } from 'node:test';

const homepage = readFileSync(new URL('../src/pages/index.astro', import.meta.url), 'utf8');

test('homepage describes the terminal without making Omarchy an exclusive platform', () => {
  const prose = homepage.replace(/<[^>]*>/g, ' ');
  assert.doesNotMatch(homepage, /a terminal for Omarchy/i);
  assert.doesNotMatch(prose, /(?:built|available)\s+for Omarchy/i);
  assert.doesNotMatch(prose, /other Linux desktops are not yet supported/i);
  assert.match(homepage, /description="A native Wayland terminal/);
  assert.match(prose, /Tested on x86_64 Arch and Omarchy with Hyprland/);
  assert.match(prose, /Follows Omarchy’s colors/);
});
