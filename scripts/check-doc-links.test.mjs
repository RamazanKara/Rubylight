import assert from 'node:assert/strict';
import { mkdirSync, mkdtempSync, rmSync, writeFileSync } from 'node:fs';
import { tmpdir } from 'node:os';
import { join, resolve, sep } from 'node:path';
import test from 'node:test';
import { checkDocs } from './check-doc-links.mjs';

function fixture(t, readme, page = '# A page\n\n## Repeated\n\n## Repeated\n\n<a id="explicit"></a>\n') {
  const parent = resolve(tmpdir());
  const root = mkdtempSync(join(parent, 'nightjar-doc-links-'));
  t.after(() => {
    assert.ok(resolve(root).startsWith(parent + sep + 'nightjar-doc-links-'));
    rmSync(root, { recursive: true });
  });
  mkdirSync(join(root, 'docs'));
  writeFileSync(join(root, 'README.md'), readme);
  writeFileSync(join(root, 'docs', 'page.md'), page);
  writeFileSync(join(root, 'docs', 'screen shot.png'), '');
  return checkDocs(root);
}

test('relative files, images, encoded spaces and duplicate/explicit anchors', t => {
  const result = fixture(t, '[page](docs/page.md#repeated-1)\n[anchor](docs/page.md#explicit)\n![image](docs/screen%20shot.png)\n', '# A page\n\n## Repeated\n\n## Repeated\n\n<a id="explicit"></a>\n[home](../README.md)\n');
  assert.deepEqual(result.errors, []);
  assert.equal(result.local, 4);
});

test('missing paths, wrong case on Windows, fragments and escaped repository paths fail', t => {
  const result = fixture(t, '[missing](docs/no.md)\n[case](docs/Page.md)\n[heading](docs/page.md#absent)\n[outside](../outside.md)\n');
  assert.equal(result.errors.length, 4);
  assert.match(result.errors[2], /missing heading/);
});

test('reference links, collapsed/shortcut references and undefined references', t => {
  const result = fixture(t, '[one][p]\n[p][]\n[p]\n[broken][missing]\n[p]: docs/page.md\n');
  assert.equal(result.errors.length, 1);
  assert.match(result.errors[0], /undefined reference \[missing\]/);
});

test('code examples are ignored, HTML images and external URLs are handled', t => {
  const result = fixture(t, '~~~md\n[example](not-a-file.md)\n~~~\n`[inline](not-a-file.md)`\n<img src="docs/screen%20shot.png">\n[web](https://example.com/path)\n');
  assert.deepEqual(result.errors, []);
  assert.equal(result.local, 1);
  assert.equal(result.external, 1);
});

test('empty links and malformed URL encoding fail', t => {
  const result = fixture(t, '[empty]()\n[bad](docs/%ZZ.md)\n');
  assert.equal(result.errors.length, 2);
});
