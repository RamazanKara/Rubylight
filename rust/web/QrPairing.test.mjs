import assert from 'node:assert/strict';
import { readFileSync } from 'node:fs';
import { test } from 'node:test';

const source = (path) => readFileSync(new URL(`./src/${path}`, import.meta.url), 'utf8').replace(/\s+/g, ' ');

test('Open in Nightjar is the same link as Copy pairing link, beside it', () => {
  const qr = source('components/devices/QrPairing.svelte');
  assert.match(qr, /<CopyButton value=\{code\.uri\} label="Copy pairing link" what="the pairing link" showLabel \/> <Button size="sm" href=\{code\.uri\}>Open in Nightjar<\/Button>/);
});

test('the console router leaves a link to another origin to the browser', () => {
  // A rubylight: URL has the origin "null", so the click reaches the system's handler.
  assert.equal(new URL('rubylight://pair?v=1&host=pc').origin, 'null');
  assert.match(source('lib/router.svelte.ts'), /if \(url\.origin !== location\.origin \|\|/);
});
