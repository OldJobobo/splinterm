import assert from 'node:assert/strict';
import { readFileSync } from 'node:fs';
import test from 'node:test';
import { checkBuiltSite, checkMetadata, criticalRoutes } from './check-release.mjs';

const release = JSON.parse(readFileSync(new URL('../src/data/release.json', import.meta.url), 'utf8'));

function fixture() {
  const html = `<html><head><meta name="splinterm-release" content="${release.version}"><meta name="description" content="A native terminal"></head><body><p>Splinterm ${release.version}</p><p>${release.packageVersion}</p><a href="${release.releaseUrl}">Release</a></body></html>`;
  return {
    pages: new Map(criticalRoutes.map((route) => [route, html])),
    sitemap: criticalRoutes.map((route) => `<loc>https://splinterm.com${route}</loc>`).join('\n'),
  };
}

function withCopy(copy) {
  const { pages, sitemap } = fixture();
  pages.set('/docs/install/', pages.get('/docs/install/').replace('</body>', `${copy}</body>`));
  return checkBuiltSite(pages, sitemap, release);
}

test('accepts a complete coherent release site', () => {
  const { pages, sitemap } = fixture();
  assert.deepEqual(checkBuiltSite(pages, sitemap, release), []);
});

test('rejects mismatched release metadata', () => {
  for (const update of [
    { tag: 'v9.9.9' }, { packageVersion: '9.9.9-1' }, { releaseUrl: 'https://example.com' },
    { channel: 'beta' }, { sourceCommit: 'main' }, { publicationRecord: '258b051' },
  ]) assert.ok(checkMetadata({ ...release, ...update }).length);
});

test('requires critical pages even if no local link points at them', () => {
  const { pages, sitemap } = fixture();
  pages.delete('/docs/packaging/');
  assert.ok(checkBuiltSite(pages, sitemap, release).some((failure) => failure.includes('missing critical route')));
});

test('requires the roadmap in the custom sitemap', () => {
  const { pages, sitemap } = fixture();
  assert.ok(checkBuiltSite(pages, sitemap.replace('<loc>https://splinterm.com/roadmap/</loc>', ''), release)
    .some((failure) => failure.includes('/roadmap/: missing from sitemap')));
});

test('rejects stale, absent, and duplicate generated version markers', () => {
  for (const replacement of ['', '<meta name="splinterm-release" content="9.9.9">',
    `<meta name="splinterm-release" content="${release.version}"><meta name="splinterm-release" content="${release.version}">`]) {
    const { pages, sitemap } = fixture();
    pages.set('/docs/', pages.get('/docs/').replace(/<meta name="splinterm-release"[^>]*>/, replacement));
    assert.ok(checkBuiltSite(pages, sitemap, release).some((failure) => failure.includes('release marker')));
  }
});

test('finds obsolete maturity claims across inline markup and entities', () => {
  for (const copy of ['<p>Public <strong>beta</strong></p>', '<a>Install the alpha</a>',
    '<p>Public&#32;alpha</p>', '<p>v0.1.0-rc.1 is the current public prerelease.</p>']) {
    assert.ok(withCopy(copy).some((failure) => failure.includes('stale current-release')));
  }
});

test('checks search descriptions as well as body copy', () => {
  const { pages, sitemap } = fixture();
  pages.set('/docs/', pages.get('/docs/').replace('content="A native terminal"', 'content="Install the public beta"'));
  assert.ok(checkBuiltSite(pages, sitemap, release).some((failure) => failure.includes('stale current-release')));
});

test('rejects obsolete edge-installer claims but allows explicit exclusions', () => {
  assert.ok(withCopy('<p>This installs the newest edge-package.</p>').length);
  assert.ok(withCopy('<p>Use the edge-package installer.</p>').length);
  assert.deepEqual(withCopy('<p>The installer never selects historical <code>edge-*</code> releases.</p>'), []);
});

test('allows explicitly historical release copy, without exempting the rest of the page', () => {
  const history = '<aside data-release-history><p>We entered public alpha earlier.</p><a href="https://github.com/OldJobobo/splinterm/releases/tag/v0.1.0-alpha2">Old release</a></aside>';
  assert.deepEqual(withCopy(history), []);
  assert.ok(withCopy(`${history}<p>Install the beta</p>`).length);
});

test('rejects noncurrent release links outside historical copy', () => {
  assert.ok(withCopy('<a href="https://github.com/OldJobobo/splinterm/releases/tag/v9.9.9">Current release</a>')
    .some((failure) => failure.includes('noncurrent release link')));
});

test('requires visible release identity, not just metadata', () => {
  const { pages, sitemap } = fixture();
  pages.set('/', pages.get('/').replace(`<p>Splinterm ${release.version}</p>`, ''));
  assert.ok(checkBuiltSite(pages, sitemap, release).some((failure) => failure.includes('visible release version')));
});

test('rejects absent and contradictory package identities in status', () => {
  for (const replacement of ['', `${release.packageVersion} and 9.9.9-1`]) {
    const { pages, sitemap } = fixture();
    pages.set('/docs/status/', pages.get('/docs/status/').replace(release.packageVersion, replacement));
    assert.ok(checkBuiltSite(pages, sitemap, release).some((failure) => failure.includes('package version')));
  }
});
