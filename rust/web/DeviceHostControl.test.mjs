import assert from 'node:assert/strict';
import { test } from 'node:test';
import { createTestServer } from './test-server.mjs';

const server = await createTestServer({ configFile: false });
const { toDraft, changes, savingEndsStream } = await server.ssrLoadModule('/src/components/devices/device.ts');
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

test('saving warns about ending the stream only when the device is cut off', () => {
  const draft = toDraft(client({}));
  const edit = patch => ({ ...draft, ...patch });
  // Host control and other edits apply later, so they don't end a stream.
  assert.equal(savingEndsStream(true, draft, edit({ host_control: true })), false);
  assert.equal(savingEndsStream(true, draft, edit({ name: 'Tablet' })), false);
  assert.equal(savingEndsStream(true, draft, edit({ perm: 0x071f1f00 | 0x80000000 })), false);
  assert.equal(savingEndsStream(true, draft, edit({ enabled: true })), false);
  // Turning the device off or removing a permission bit does.
  assert.equal(savingEndsStream(true, draft, edit({ enabled: false })), true);
  assert.equal(savingEndsStream(true, draft, edit({ perm: draft.perm & ~0x100 })), true);
  // Nothing to end when the device isn't streaming, or was already off.
  assert.equal(savingEndsStream(false, draft, edit({ enabled: false })), false);
  const off = toDraft(client({ enabled: false }));
  assert.equal(savingEndsStream(true, off, { ...off, name: 'Tablet' }), false);
});

test('sleep is off unless the device holds a JSON true', () => {
  assert.equal(toDraft(client({})).allow_sleep, false);
  assert.equal(toDraft(client({ perm: 0xffffffff, host_control: true })).allow_sleep, false);
  assert.equal(toDraft(client({ allow_sleep: true })).allow_sleep, true);
  for (const value of [false, null, 1, '1', 'true', [true]]) {
    assert.equal(toDraft(client({ allow_sleep: value })).allow_sleep, false, String(value));
  }
});

test('the sleep draft round-trips: true to grant, null to revoke', () => {
  const off = toDraft(client({}));
  const on = { ...off, allow_sleep: true };
  assert.deepEqual(changes(off, on), { allow_sleep: true });
  assert.deepEqual(changes(on, off), { allow_sleep: null });
  const stored = toDraft(client({ allow_sleep: true }));
  assert.deepEqual(stored, on);
  assert.deepEqual(changes(stored, { ...stored, name: 'Tablet' }), { name: 'Tablet' });
  assert.equal(savingEndsStream(true, off, on), false);
});