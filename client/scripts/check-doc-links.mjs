import { existsSync, readFileSync, readdirSync, statSync } from 'node:fs';
import { dirname, isAbsolute, join, relative, resolve, sep } from 'node:path';
import { fileURLToPath } from 'node:url';

function withoutCode(text) {
  let fence;
  return text.split('\n').map(line => {
    const marker = line.match(/^\s{0,3}(\x60{3,}|~{3,})/);
    if (marker) {
      if (!fence) fence = marker[1];
      else if (marker[1][0] === fence[0] && marker[1].length >= fence.length) fence = undefined;
      return '';
    }
    return fence ? '' : line;
  }).join('\n');
}

function anchors(text) {
  const result = new Set();
  const counts = new Map();
  const clean = withoutCode(text);
  const headings = [...clean.matchAll(/^ {0,3}#{1,6}\s+(.+?)(?:\s+#+)?\s*$/gm)].map(m => m[1]);
  for (const match of clean.matchAll(/^([^\n]+)\n(?:={3,}|-{3,})\s*$/gm)) headings.push(match[1]);
  for (const heading of headings) {
    const slug = heading.replace(/<[^>]*>/g, '').replace(/\[([^\]]+)\]\([^)]*\)/g, '$1')
      .replace(/&amp;/g, '&').toLowerCase().replace(/[^\p{L}\p{N}\p{M}_\-\s]/gu, '').replace(/\s/g, '-');
    const count = counts.get(slug) ?? 0;
    result.add(slug + (count ? '-' + count : ''));
    counts.set(slug, count + 1);
  }
  for (const match of clean.matchAll(/<(?:a|h[1-6])\b[^>]*\b(?:id|name)=["']([^"']+)["']/gi)) result.add(match[1]);
  return result;
}

function links(text) {
  const references = new Map();
  const targets = [];
  const errors = [];
  const key = label => label.trim().replace(/\s+/g, ' ').toLowerCase();
  let clean = withoutCode(text).replace(/<!--[\s\S]*?-->/g, '');
  clean = clean.replace(/^\s{0,3}\[([^\]]+)\]:\s*(?:<([^>]+)>|(\S+)).*$/gm, (_, label, angle, plain) => {
    references.set(key(label), angle ?? plain);
    targets.push(angle ?? plain);
    return '';
  });
  clean = clean.replace(/\x60+([^\x60]*?)\x60+/g, '');
  clean = clean.replace(/!?\[[^\]]*\]\(\s*(?:<([^>]+)>|((?:[^()\s]|\([^()]*\))*))(?:\s+["'][^"']*["'])?\s*\)/g, (_, angle, plain) => {
    targets.push(angle ?? plain);
    return '';
  });
  clean = clean.replace(/!?\[([^\]]+)\]\[([^\]]*)\]/g, (_, label, ref) => {
    const target = references.get(key(ref || label));
    if (target === undefined) errors.push('undefined reference [' + (ref || label) + ']');
    else targets.push(target);
    return '';
  });
  for (const match of clean.matchAll(/!?\[([^\]]+)\]/g)) {
    if (references.has(key(match[1]))) targets.push(references.get(key(match[1])));
  }
  for (const match of clean.matchAll(/\b(?:href|src)=["']([^"']+)["']/gi)) targets.push(match[1]);
  for (const match of clean.matchAll(/<(https?:\/\/[^>]+)>/g)) targets.push(match[1]);
  return { targets, errors };
}

function markdownFiles(directory) {
  return readdirSync(directory, { withFileTypes: true }).flatMap(entry => {
    const file = join(directory, entry.name);
    return entry.isDirectory() ? markdownFiles(file) : entry.name.endsWith('.md') ? [file] : [];
  });
}

function exactPath(root, target) {
  const local = relative(root, target);
  if (local === '..' || local.startsWith('..' + sep) || isAbsolute(local)) return false;
  let current = root;
  for (const part of local.split(sep).filter(Boolean)) {
    if (!existsSync(current) || !statSync(current).isDirectory() || !readdirSync(current).includes(part)) return false;
    current = join(current, part);
  }
  return existsSync(target);
}

export function checkDocs(root) {
  root = resolve(root);
  const files = [join(root, 'README.md'), ...markdownFiles(join(root, 'docs'))];
  const errors = [];
  let local = 0;
  let external = 0;
  for (const file of files) {
    const found = links(readFileSync(file, 'utf8'));
    const report = message => errors.push(relative(root, file).split(sep).join('/') + ': ' + message);
    found.errors.forEach(report);
    for (const target of found.targets) {
      if (!target) {
        report('empty link');
        continue;
      }
      if (/^[a-z][a-z\d+.-]*:/i.test(target) || target.startsWith('//')) {
        try {
          new URL(target, 'https://example.invalid');
          external++;
        } catch {
          report('invalid URL: ' + target);
        }
        continue;
      }
      local++;
      try {
        const [pathAndQuery, fragment] = target.split('#', 2);
        const path = decodeURIComponent(pathAndQuery.split('?', 1)[0]);
        const destination = path ? resolve(path.startsWith('/') ? root : dirname(file), path.replace(/^\/+/, '')) : file;
        if (!exactPath(root, destination)) {
          report('missing path or wrong filename case: ' + target);
        } else if (fragment && destination.endsWith('.md') && !anchors(readFileSync(destination, 'utf8')).has(decodeURIComponent(fragment))) {
          report('missing heading/anchor: ' + target);
        }
      } catch (error) {
        report(target + ': ' + error.message);
      }
    }
  }
  return { files: files.length, local, external, errors };
}

if (process.argv[1] && resolve(process.argv[1]) === fileURLToPath(import.meta.url)) {
  const result = checkDocs(fileURLToPath(new URL('..', import.meta.url)));
  if (result.errors.length) {
    result.errors.forEach(error => console.error(error));
    process.exitCode = 1;
  } else {
    console.log('Checked ' + result.files + ' Markdown files and ' + result.local + ' local links; no broken paths or anchors.');
    console.log(result.external + ' external URLs syntax-checked only; no network requests.');
  }
}
