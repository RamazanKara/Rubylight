import assert from 'node:assert/strict';
import { test } from 'node:test';
import { createTestServer } from './test-server.mjs';

const server = await createTestServer({ configFile: false });
const { ACCESS_CHOICES, ACCESS_PERM, accessOf, accessLabel, withAccess } = await server.ssrLoadModule('/src/components/devices/access.ts');

test('the presets are the host masks', () => {
  // butterpollo_core::state: PLAY_DEVICE_PERMISSION, LATER_DEVICE_PERMISSION, FIRST_DEVICE_PERMISSION.
  assert.deepEqual(ACCESS_PERM, { play: 0x07001f00, view: 0x03000000, all: 0x07131f00 });
  assert.deepEqual(ACCESS_CHOICES.map((choice) => choice.id), ['play', 'view', 'all']);
  assert.equal(ACCESS_CHOICES[0].label, 'Play games');
});

test('a preset reads back as itself', () => {
  for (const { id } of ACCESS_CHOICES) {
    assert.equal(accessOf(withAccess(0, id)), id);
    assert.equal(accessOf(withAccess(0x071f1f00, id)), id);
  }
  assert.equal(accessLabel(0x07001f00), 'Play games');
  assert.equal(accessLabel(0x03000000), 'Watch only');
  // Bits the console does not know are ignored, as the editor drops them.
  assert.equal(accessOf(0x07001f00 | 0x10000000), 'play');
});

test('file transfer keeps Play games and Everything, and Watch only takes it away', () => {
  assert.equal(withAccess(0x01040000, 'play'), 0x07041f00);
  assert.equal(accessOf(0x07041f00), 'play');
  assert.equal(withAccess(0x071f1f00, 'view'), 0x03000000);
  assert.equal(accessOf(0x03040000), 'custom');
});

test('any other mask is Custom, and none is No access', () => {
  for (const perm of [0x01000000, 0x07000000, 0x07011f00, 0x03001f00, 0x07001e00]) {
    assert.equal(accessOf(perm), 'custom', perm.toString(16));
    assert.equal(accessLabel(perm), 'Custom');
  }
  assert.equal(accessOf(0), 'none');
  assert.equal(accessLabel(0x00040000), 'Custom');
  assert.equal(accessLabel(0), 'No access');
});
