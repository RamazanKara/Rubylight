import assert from 'node:assert/strict';
import { readFileSync } from 'node:fs';
import { test } from 'node:test';
import { createTestServer } from './test-server.mjs';

const server = await createTestServer({ configFile: false });
const { toDraft, changes, savingEndsStream, PERMISSION_GROUPS } = await server.ssrLoadModule('/src/components/devices/device.ts');
const { withAccess, accessLabel } = await server.ssrLoadModule('/src/components/devices/access.ts');
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
test('restart and shut down is its own grant: off unless a JSON true, and round-trips', () => {
  assert.equal(toDraft(client({})).allow_restart_shutdown, false);
  // Sleep PC, Host control and every permission bit leave it off.
  const others = toDraft(client({ perm: 0xffffffff, host_control: true, allow_sleep: true }));
  assert.equal(others.allow_restart_shutdown, false);
  for (const value of [false, null, 1, 'true', [true]]) {
    assert.equal(toDraft(client({ allow_restart_shutdown: value })).allow_restart_shutdown, false, String(value));
  }
  const off = toDraft(client({}));
  const on = { ...off, allow_restart_shutdown: true };
  assert.equal(on.allow_sleep, false);
  assert.deepEqual(changes(off, on), { allow_restart_shutdown: true });
  assert.deepEqual(changes(on, off), { allow_restart_shutdown: null });
  assert.deepEqual(toDraft(client({ allow_restart_shutdown: true })), on);
  assert.equal(savingEndsStream(true, off, on), false);
});
test('file transfer bits are unlabelled and round-trip in the draft', () => {
  const items = PERMISSION_GROUPS.flatMap(group => group.items);
  assert.equal(items.find(item => item.bit === 0x40000).label, 'Upload files');
  assert.equal(items.find(item => item.bit === 0x80000).label, 'Download files');
  const full = toDraft(client({}));
  assert.equal(full.perm & 0xc0000, 0xc0000);
  const uploadOnly = { ...full, perm: full.perm & ~0x80000 };
  assert.deepEqual(changes(full, uploadOnly), { perm: 0x07171f00 });
  assert.deepEqual(toDraft(client({ perm: 0x07171f00 })), uploadOnly);
  // Losing a bit ends a running stream, as any revoke does.
  assert.equal(savingEndsStream(true, full, uploadOnly), true);
});
test('everything leaves the file transfer bits as they are', () => {
  assert.equal(withAccess(0x01000000, 'all'), 0x07131f00);
  assert.equal(withAccess(0x01040000, 'all'), 0x07171f00);
  assert.equal(withAccess(0x071f1f00, 'all'), 0x071f1f00);
  assert.equal(accessLabel(0x07131f00), 'Everything');
  assert.equal(accessLabel(0x071f1f00), 'Everything');
  assert.equal(accessLabel(0x07031f00), 'Custom');
});

test('the device pages never say the first device gets everything: Restart and shut down PC is its own grant', () => {
  for (const page of ['DeviceEditor.svelte', 'PairPanel.svelte']) {
    const text = readFileSync(new URL(`./src/components/devices/${page}`, import.meta.url), 'utf8').replace(/\s+/g, ' ');
    assert.doesNotMatch(text, /first (paired device|device paired to this PC) (gets|can do) everything/, page);
  }
  const editor = readFileSync(new URL('./src/components/devices/DeviceEditor.svelte', import.meta.url), 'utf8').replace(/\s+/g, ' ');
  assert.match(editor, /File transfer and Restart and shut down PC are granted on their own, never with Everything\./);
});
