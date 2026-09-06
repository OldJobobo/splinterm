import assert from 'node:assert/strict';
import { readFileSync } from 'node:fs';
import { isAbsolute, relative, resolve, sep } from 'node:path';
import { fileURLToPath } from 'node:url';

function attributes(tag) {
  return Object.fromEntries([...tag.matchAll(/([\w:-]+)=["']([^"']*)["']/g)].map((match) => [match[1], match[2]]));
}

export function checkFont({
  root = resolve('dist'),
  source = resolve('../assets/fonts/splinter-display/dist/SplinterDisplay-Heavy.woff2'),
} = {}) {
  function publicFile(url) {
    assert.ok(url.startsWith('/') && !url.startsWith('//'), `Expected a local asset URL: ${url}`);
    const target = resolve(root, `.${decodeURIComponent(url.split(/[?#]/)[0])}`);
    const pathFromRoot = relative(root, target);
    assert.ok(!isAbsolute(pathFromRoot) && pathFromRoot !== '..' && !pathFromRoot.startsWith(`..${sep}`),
      `Asset escapes dist/: ${url}`);
    return target;
  }
  const html = readFileSync(resolve(root, 'index.html'), 'utf8');
  const links = [...html.matchAll(/<link\b[^>]*>/g)].map((match) => attributes(match[0]));
  const preloads = links.filter((link) => link.rel === 'preload' && link.as === 'font');
  assert.equal(preloads.length, 1, 'Homepage must preload exactly one display font');
  const [preload] = preloads;
  assert.equal(preload.type, 'font/woff2');
  assert.equal(preload.crossorigin, 'anonymous');
  assert.ok(readFileSync(publicFile(preload.href)).equals(readFileSync(source)),
    'Bundled font differs from the canonical Splinter Display asset');

  const css = links.filter((link) => link.rel === 'stylesheet')
    .map((link) => readFileSync(publicFile(link.href), 'utf8')).join('\n');
  const face = [...css.matchAll(/@font-face\s*\{([^}]+)\}/g)]
    .map((match) => match[1]).find((rule) => /font-family:\s*["']?Splinter Display/.test(rule));
  assert.ok(face, 'Splinter Display @font-face is missing from homepage CSS');
  assert.ok(face.includes(preload.href), 'Preload and CSS must use the same font URL');
  assert.match(face, /font-weight:\s*900/);
  assert.match(face, /font-display:\s*swap/);
  const wordmark = [...css.matchAll(/\.intro-wordmark\s*\{([^}]+)\}/g)]
    .map((match) => match[1]).find((rule) => rule.includes('font-family'));
  assert.ok(wordmark, 'Wordmark typography rule is missing');
  assert.match(css, /--display:\s*["']?Splinter Display/);
  assert.match(wordmark, /font-family:\s*var\(--display\)/);
  assert.match(wordmark, /font-weight:\s*900/);
  assert.match(wordmark, /font-synthesis:\s*none/);
  assert.match(wordmark, /font-kerning:\s*normal/);
  assert.match(wordmark, /letter-spacing:\s*0(?:;|$)/);
  const heading = html.match(/<h1\b[^>]*\bid="intro-title"[^>]*>([\s\S]*?)<\/h1>/)?.[1];
  assert.ok(heading, 'Wordmark heading is missing');
  assert.equal(heading.replace(/<[^>]*>/g, ''), 'splinterm');

  for (const page of ['roadmap/index.html', 'docs/index.html']) {
    const pageHtml = readFileSync(resolve(root, page), 'utf8');
    const fontPreloads = [...pageHtml.matchAll(/<link\b[^>]*>/g)]
      .map((match) => attributes(match[0])).filter((link) => link.rel === 'preload' && link.as === 'font');
    assert.ok(!fontPreloads.some((link) => link.href === preload.href), `${page} needlessly preloads the wordmark font`);
  }
  return preload.href;
}

if (process.argv[1] && resolve(process.argv[1]) === fileURLToPath(import.meta.url)) {
  try {
    console.log(`Verified canonical wordmark font, preload, and typography: ${checkFont()}`);
  } catch (error) {
    console.error(`Wordmark font check failed: ${error.message}`);
    process.exitCode = 1;
  }
}
