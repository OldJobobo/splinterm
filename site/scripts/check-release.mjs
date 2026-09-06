import { readFileSync, readdirSync } from 'node:fs';
import { join, relative, resolve, sep } from 'node:path';
import { pathToFileURL } from 'node:url';

export const criticalRoutes = [
  '/', '/roadmap/', '/docs/', '/docs/status/', '/docs/install/',
  '/docs/packaging/', '/docs/quickstart/', '/docs/concepts/', '/docs/sessions/',
  '/docs/configure/configuration/', '/docs/troubleshooting/', '/docs/mcp/',
];

function decode(value) {
  const named = { amp: '&', lt: '<', gt: '>', quot: '"', apos: "'", nbsp: ' ' };
  return value.replace(/&(#x[\da-f]+|#\d+|amp|lt|gt|quot|apos|nbsp);/gi, (entity, key) => {
    if (!key.startsWith('#')) return named[key.toLowerCase()];
    const point = key[1].toLowerCase() === 'x'
      ? Number.parseInt(key.slice(2), 16) : Number.parseInt(key.slice(1), 10);
    return point > 0 && point <= 0x10ffff ? String.fromCodePoint(point) : entity;
  });
}

function attributes(tag) {
  return Object.fromEntries([...tag.matchAll(/([\w-]+)\s*=\s*(?:"([^"]*)"|'([^']*)')/g)]
    .map((match) => [match[1].toLowerCase(), decode(match[2] ?? match[3])]));
}

function currentContent(html) {
  // Historical copy must be explicitly scoped, never a page-wide exemption.
  return html.replace(/<aside\b[^>]*\bdata-release-history(?:\s|=|>)[\s\S]*?<\/aside>/gi, '')
    .replace(/<(script|style)\b[^>]*>[\s\S]*?<\/\1>/gi, '')
    .replace(/<!--[\s\S]*?-->/g, '');
}

function textContent(html) {
  return decode(html.replace(/<[^>]+>/g, ' ')).replace(/\s+/g, ' ').trim();
}

export function checkMetadata(release) {
  const failures = [];
  if (!/^\d+\.\d+\.\d+$/.test(release.version ?? '') || release.channel !== 'stable') {
    failures.push('release metadata must identify the reviewed stable SemVer baseline');
  }
  if (release.tag !== `v${release.version}`) failures.push('release tag/version disagree');
  if (!new RegExp(`^${String(release.version).replace(/[.*+?^${}()|[\]\\]/g, '\\$&')}-\\d+$`).test(release.packageVersion ?? '')) {
    failures.push('package version must match the release version plus Arch pkgrel');
  }
  if (release.releaseUrl !== `https://github.com/OldJobobo/splinterm/releases/tag/${release.tag}`) {
    failures.push('release URL/tag disagree');
  }
  for (const key of ['sourceCommit', 'publicationRecord']) {
    if (!/^[a-f0-9]{40}$/.test(release[key] ?? '')) failures.push(`${key} must be an exact commit`);
  }
  return failures;
}

export function checkBuiltSite(pages, sitemap, release) {
  const failures = checkMetadata(release);
  for (const route of criticalRoutes) {
    if (!pages.has(route)) failures.push(`${route}: missing critical route`);
    if (!sitemap.includes(`<loc>https://splinterm.com${route}</loc>`)) {
      failures.push(`${route}: missing from sitemap.xml`);
    }
  }

  for (const [route, html] of pages) {
    const content = currentContent(html);
    const metas = [...html.matchAll(/<meta\b[^>]*>/gi)].map(([tag]) => attributes(tag));
    const markers = metas.filter((meta) => meta.name === 'splinterm-release');
    if (markers.length !== 1 || markers[0].content !== release.version) {
      failures.push(`${route}: missing or contradictory splinterm-release marker`);
    }
    const prose = `${textContent(content)} ${metas.filter((meta) => meta.name === 'description').map((meta) => meta.content).join(' ')}`;
    const staleClaims = [
      /\bpublic\s+(?:alpha|beta)\b/i,
      /\binstall\s+(?:the\s+)?(?:alpha|beta)\b/i,
      /\b(?:current|latest)\s+public\s+prerelease\b/i,
      /\bedge[- ]package\s+installer\b/i,
      /\b(?:installs?|downloads?|selects?)\s+(?:(?:the|latest|newest|current)\s+)*edge[- ](?:package|release|build)/i,
      /\bv?\d+\.\d+\.\d+[-.]?(?:alpha|beta|rc)[.\d-]*\b/i,
    ];
    for (const pattern of staleClaims) {
      if (pattern.test(prose)) failures.push(`${route}: stale current-release claim (${prose.match(pattern)[0]})`);
    }
    for (const [, tag] of content.matchAll(/https:\/\/github\.com\/OldJobobo\/splinterm\/releases\/tag\/([^"'<>\s]+)/g)) {
      if (decode(tag) !== release.tag) failures.push(`${route}: noncurrent release link ${tag}; mark historical copy explicitly`);
    }
  }

  for (const route of ['/', '/docs/status/']) {
    const html = currentContent(pages.get(route) ?? '');
    if (!textContent(html).includes(`Splinterm ${release.version}`) || !html.includes(release.releaseUrl)) {
      failures.push(`${route}: missing visible release version or release link`);
    }
  }
  const status = textContent(currentContent(pages.get('/docs/status/') ?? ''));
  if (!status.includes(release.packageVersion)) failures.push('/docs/status/: missing current package version');
  for (const version of status.match(/\b\d+\.\d+\.\d+(?:[a-z]+\d*|-[\w.]+)?-\d+\b/g) ?? []) {
    if (version !== release.packageVersion) failures.push(`/docs/status/: contradictory package version ${version}`);
  }
  return failures;
}

function walk(directory) {
  return readdirSync(directory, { withFileTypes: true }).flatMap((entry) => {
    const path = join(directory, entry.name);
    return entry.isDirectory() ? walk(path) : [path];
  });
}

if (process.argv[1] && import.meta.url === pathToFileURL(resolve(process.argv[1])).href) {
  try {
    const root = resolve('dist');
    const release = JSON.parse(readFileSync(new URL('../src/data/release.json', import.meta.url), 'utf8'));
    const pages = new Map(walk(root).filter((file) => file.endsWith('.html')).map((file) => {
      const path = relative(root, file).split(sep).join('/');
      return [`/${path.replace(/index\.html$/, '')}`, readFileSync(file, 'utf8')];
    }));
    const failures = checkBuiltSite(pages, readFileSync(join(root, 'sitemap.xml'), 'utf8'), release);
    if (failures.length) throw new Error(failures.join('\n'));
    console.log(`Release ${release.tag}: checked ${pages.size} pages and ${criticalRoutes.length} critical routes.`);
  } catch (error) {
    console.error(`Release check failed (run npm run build first):\n${error.message}`);
    process.exitCode = 1;
  }
}
