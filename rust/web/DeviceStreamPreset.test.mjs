import assert from 'node:assert/strict';
import { test } from 'node:test';
import { createTestServer } from './test-server.mjs';

const server = await createTestServer({ configFile: false });
const { toDraft, changes } = await server.ssrLoadModule('/src/components/devices/device.ts');
const client = extra => ({ uuid: 'u', name: 'Phone', perm: 0x071f1f00, enabled: true, connected: false, ...extra });

test('a device with no overrides follows the host preset and sends nothing', () => {
  const draft = toDraft(client({}));
  assert.equal(draft.stream_preset, '');
  assert.deepEqual(changes(draft, { ...draft }), {});
  // An empty overrides object is the same as none.
  assert.deepEqual(changes(toDraft(client({ config_overrides: {} })), draft), {});
});

test('the stored preset is read as the host reads it, in any spelling', () => {
  for (const value of ['reference', 'Reference', ' "Lowest-Latency" ']) {
    const id = value.replace(/["\s]/g, '').toLowerCase().replace('-', '_');
    assert.equal(toDraft(client({ config_overrides: { stream_preset: value } })).stream_preset, id, value);
  }
});

test('choosing a preset keeps the device other overrides', () => {
  const before = toDraft(client({ config_overrides: { max_bitrate: 20000, host_adaptive_pacing: 'enabled' } }));
  const after = { ...before, stream_preset: 'lowest_latency' };
  assert.deepEqual(changes(before, after), {
    config_overrides: { max_bitrate: 20000, host_adaptive_pacing: 'enabled', stream_preset: 'lowest_latency' },
  });
});

test('going back to the host preset removes the key, and the last override clears the object', () => {
  const before = toDraft(client({ config_overrides: { stream_preset: 'reference', max_bitrate: 5 } }));
  assert.deepEqual(changes(before, { ...before, stream_preset: '' }), { config_overrides: { max_bitrate: 5 } });
  const only = toDraft(client({ config_overrides: { stream_preset: 'reference' } }));
  assert.deepEqual(changes(only, { ...only, stream_preset: '' }), { config_overrides: null });
});

test('an unknown saved name is kept until the user picks a preset', () => {
  const before = toDraft(client({ config_overrides: { stream_preset: 'fastest' } }));
  assert.equal(before.stream_preset, 'fastest');
  assert.deepEqual(changes(before, { ...before, name: 'Tablet' }), { name: 'Tablet' });
  assert.deepEqual(changes(before, { ...before, stream_preset: 'balanced' }), {
    config_overrides: { stream_preset: 'balanced' },
  });
});
