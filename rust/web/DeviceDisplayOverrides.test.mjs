import assert from 'node:assert/strict';
import { test } from 'node:test';
import { createTestServer } from './test-server.mjs';

const server = await createTestServer({ configFile: false });
const { toDraft, changes, DISPLAY_OVERRIDE_OPTIONS } = await server.ssrLoadModule('/src/components/devices/device.ts');
const client = extra => ({ uuid: 'u', name: 'Phone', perm: 0x071f1f00, enabled: true, connected: false, ...extra });

test('a device follows the host rotation and scale until it sets its own', () => {
  const draft = toDraft(client({}));
  assert.deepEqual(draft.display, { dd_rotation_option: '', dd_manual_rotation: '', dd_virtual_display_scale: '' });
  assert.deepEqual(changes(draft, { ...draft }), {});
});

test('the selects offer what the host settings offer', () => {
  assert.deepEqual(DISPLAY_OVERRIDE_OPTIONS.dd_rotation_option.map(o => o.value), ['disabled', 'auto', 'manual']);
  assert.deepEqual(DISPLAY_OVERRIDE_OPTIONS.dd_manual_rotation.map(o => o.value), ['0', '90', '180', '270']);
  assert.deepEqual(DISPLAY_OVERRIDE_OPTIONS.dd_virtual_display_scale.slice(0, 2).map(o => o.value), ['0', '-1']);
});

test('choosing a rotation and a scale keeps the device other overrides and preset', () => {
  const before = toDraft(client({ config_overrides: { max_bitrate: 20000, stream_preset: 'balanced' } }));
  const after = { ...before, display: { dd_rotation_option: 'manual', dd_manual_rotation: '90', dd_virtual_display_scale: '150' } };
  assert.deepEqual(changes(before, after), {
    config_overrides: {
      max_bitrate: 20000,
      dd_rotation_option: 'manual',
      dd_manual_rotation: 90,
      dd_virtual_display_scale: 150,
      stream_preset: 'balanced',
    },
  });
});

test('stored values are read back, and going back to the host setting removes them', () => {
  const before = toDraft(client({ config_overrides: { dd_rotation_option: 'Auto', dd_virtual_display_scale: -1 } }));
  assert.equal(before.display.dd_rotation_option, 'auto');
  assert.equal(before.display.dd_virtual_display_scale, '-1');
  assert.equal('dd_rotation_option' in before.other_overrides, false);
  // Untouched, they go back as stored, spelling included.
  assert.deepEqual(changes(before, { ...before, name: 'Tablet' }), { name: 'Tablet' });
  const cleared = { ...before, display: { ...before.display, dd_rotation_option: '' } };
  assert.deepEqual(changes(before, cleared), { config_overrides: { dd_virtual_display_scale: -1 } });
  const none = { ...cleared, display: { ...cleared.display, dd_virtual_display_scale: '' } };
  assert.deepEqual(changes(before, none), { config_overrides: null });
});

test('a stored value the console cannot show goes back as stored', () => {
  for (const stored of ['sideways', 45, true]) {
    const before = toDraft(client({ config_overrides: { dd_manual_rotation: stored } }));
    assert.deepEqual(changes(before, { ...before, name: 'Tablet' }), { name: 'Tablet' }, String(stored));
    const picked = { ...before, display: { ...before.display, dd_manual_rotation: '180' } };
    assert.deepEqual(changes(before, picked), { config_overrides: { dd_manual_rotation: 180 } }, String(stored));
  }
});
