import assert from 'node:assert/strict';
import { test } from 'node:test';
import { createTestServer } from './test-server.mjs';

const server = await createTestServer({ configFile: false });
const { toDraft, changes } = await server.ssrLoadModule('/src/components/devices/device.ts');
const client = extra => ({ uuid: 'u', name: 'Phone', perm: 0x071f1f00, enabled: true, connected: false, ...extra });

test('host control is off for a device that never had it', () => {
  assert.equal(toDraft(client({})).host_control, false);
  // A full permission value does not grant it.
  assert.equal(toDraft(client({ perm: 0xffffffff })).host_control, false);
});

test('only true and "true" show as granted, as the host reads them', () => {
  assert.equal(toDraft(client({ host_control: true })).host_control, true);
  assert.equal(toDraft(client({ host_control: 'true' })).host_control, true);
  for (const value of [false, null, 1, '1', 'yes', 'TRUE', [true]]) {
    assert.equal(toDraft(client({ host_control: value })).host_control, false, String(value));
  }
});

test('granting sends true and revoking sends null, never a string', () => {
  const off = toDraft(client({}));
  const on = { ...off, host_control: true };
  assert.deepEqual(changes(off, on), { host_control: true });
  assert.deepEqual(changes(on, off), { host_control: null });
  assert.deepEqual(changes(off, { ...off }), {});
  // A stored "true" is the same grant, so saving other edits doesn't rewrite it.
  const stored = toDraft(client({ host_control: 'true' }));
  assert.deepEqual(changes(stored, { ...stored, name: 'Tablet' }), { name: 'Tablet' });
});
