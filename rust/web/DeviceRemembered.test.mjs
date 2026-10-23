import assert from 'node:assert/strict';
import { test } from 'node:test';
import { createTestServer } from './test-server.mjs';

const server = await createTestServer({ configFile: false });
const { toDraft, changes } = await server.ssrLoadModule('/src/components/devices/device.ts');
const client = extra => ({ uuid: 'u', name: 'Phone', perm: 0x071f1f00, enabled: true, connected: false, ...extra });

test('the kept resolution and scale show on the device', () => {
  const draft = toDraft(
    client({ remember_display_mode: true, remembered_resolution: '2560x1440', remembered_scale: 200 }),
  );
  assert.equal(draft.remembered_resolution, '2560x1440');
  assert.equal(draft.remembered_scale, '200');
  assert.equal(toDraft(client({ remember_display_mode: true })).remembered_scale, '');
});

test('turning the option off clears the kept scale with the kept resolution', () => {
  const on = toDraft(
    client({ remember_display_mode: true, remembered_resolution: '2560x1440', remembered_scale: 200 }),
  );
  assert.deepEqual(changes(on, { ...on, remember_display_mode: false }), {
    remember_display_mode: false,
    remembered_resolution: null,
    remembered_scale: null,
  });
  // Saving other edits with the option on leaves what is kept alone.
  assert.deepEqual(changes(on, { ...on, name: 'Tablet' }), { name: 'Tablet' });
});
