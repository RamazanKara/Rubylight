import assert from 'node:assert/strict';
import { readFileSync } from 'node:fs';
import { test } from 'node:test';
import { createTestServer } from './test-server.mjs';

const server = await createTestServer({ configFile: false });
const { pairLinkTarget } = await server.ssrLoadModule('/src/components/devices/pairLink.ts');

test('the toast link waits for a request, the tray link for the code', () => {
  assert.equal(pairLinkTarget('pin', 0, true), null);
  assert.equal(pairLinkTarget('pin', 1, false), 'pin');
  assert.equal(pairLinkTarget('qr', 0, false), null);
  assert.equal(pairLinkTarget('qr', 0, true), 'qr');
});

test('any other page address leads nowhere', () => {
  for (const want of [null, '', 'other', 'PIN']) assert.equal(pairLinkTarget(want, 1, true), null);
});

test('the panel acts on the link once per address, so a later request never takes the focus', () => {
  const panel = readFileSync(new URL('./src/components/devices/PairPanel.svelte', import.meta.url), 'utf8');
  assert.match(panel, /pairLinkTarget\(query\('pair'\), session\.pending\.length, qr\)/);
  assert.match(panel, /if \(!target \|\| followed === route\.search\) return;/);
  assert.match(panel, /querySelector<HTMLInputElement>\('\.requests input'\)\?\.focus\(\)/);
  assert.match(panel, /getElementById\('pair-qr'\)\?\.scrollIntoView\(\)/);
});

