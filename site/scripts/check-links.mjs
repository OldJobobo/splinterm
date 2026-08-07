import { existsSync, readFileSync, readdirSync } from 'node:fs';
import { dirname, extname, isAbsolute, join, normalize, relative, resolve, sep } from 'node:path';

const root = resolve('dist');

if (!existsSync(root)) {
  console.error('dist/ does not exist. Run npm run build first.');
  process.exit(1);
}

function walk(directory) {
  return readdirSync(directory, { withFileTypes: true }).flatMap((entry) => {
    const path = join(directory, entry.name);
    return entry.isDirectory() ? walk(path) : [path];
  });
}

function publicTarget(pathname) {
  const clean = pathname.replace(/^\//, '');
  if (!clean) return join(root, 'index.html');
  const direct = join(root, clean);
  if (existsSync(direct) && extname(direct)) return direct;
  if (existsSync(direct) && !extname(direct)) return join(direct, 'index.html');
  return extname(direct) ? direct : join(direct, 'index.html');
}

function staysInsideRoot(target) {
  const pathFromRoot = relative(root, target);
  return (
    pathFromRoot === '' ||
    (!isAbsolute(pathFromRoot) && pathFromRoot !== '..' && !pathFromRoot.startsWith(`..${sep}`))
  );
}

const htmlFiles = walk(root).filter((file) => file.endsWith('.html'));
const failures = [];
let checked = 0;

for (const file of htmlFiles) {
  const html = readFileSync(file, 'utf8');
  const links = [...html.matchAll(/\b(?:href|src)=(?:"([^"]+)"|'([^']+)')/g)].map(
    (match) => match[1] ?? match[2],
  );

  for (const link of links) {
    if (!link || /^(?:https?:|mailto:|tel:|data:|javascript:|#)/.test(link)) continue;

    const [encodedPathname] = link.split(/[?#]/, 1);
    let pathname;
    try {
      pathname = decodeURIComponent(encodedPathname);
    } catch {
      failures.push(`${relative(root, file)} -> ${link} (malformed URL encoding)`);
      continue;
    }

    let target;
    if (pathname.startsWith('/')) {
      target = publicTarget(pathname);
    } else {
      const resolved = normalize(join(dirname(relative(root, file)), pathname));
      target = resolve(root, resolved);
      if (!extname(target) && !existsSync(target)) target = join(target, 'index.html');
    }

    checked += 1;
    if (!staysInsideRoot(target)) {
      failures.push(`${relative(root, file)} -> ${link} (escapes dist/)`);
    } else if (!existsSync(target)) {
      failures.push(`${relative(root, file)} -> ${link}`);
    }
  }
}

if (failures.length) {
  console.error(`Found ${failures.length} broken local asset or page link(s):`);
  for (const failure of failures) console.error(`- ${failure}`);
  process.exit(1);
}

console.log(`Checked ${checked} local page and asset links across ${htmlFiles.length} HTML files.`);
