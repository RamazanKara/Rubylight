import assert from 'node:assert/strict';
import { test } from 'node:test';
import { createTestServer } from './test-server.mjs';

const server = await createTestServer({ configFile: false });
const { toDraft, changes } = await server.ssrLoadModule('/src/components/devices/device.ts');
const client = extra => ({ uuid: 'u', name: 'Phone', perm: 0x071f1f00, enabled: true, connected: false, ...extra });

test('a layout the host learned is not shown as the user\'s, and a rename does not make it so', () => {
  // Issue #40: the host saves the layout a stream ended in; only the user's pick is protected.
  const learned = toDraft(
    client({ virtual_display_layout: 'extended', virtual_display_layout_learned_over: 'extended_primary' }),
  );
  assert.equal(learned.virtual_display_layout_set_by_user, false);
  assert.deepEqual(changes(learned, { ...learned, name: 'Tablet' }), { name: 'Tablet' });
});

test('touching the layout field sends the user marker even when the value is the same', () => {
  const learned = toDraft(client({ virtual_display_layout: 'extended' }));
  const touched = { ...learned, virtual_display_layout_set_by_user: true };
  assert.deepEqual(changes(learned, touched), { virtual_display_layout_set_by_user: true });
  // Saved, it stays and later saves do not resend it.
  const saved = toDraft(client({ virtual_display_layout: 'extended', virtual_display_layout_set_by_user: true }));
  assert.deepEqual(changes(saved, { ...saved, name: 'Tablet' }), { name: 'Tablet' });
});

test('choosing another layout sends the marker with it, and the host setting clears both', () => {
  const learned = toDraft(client({ virtual_display_layout: 'extended' }));
  const picked = { ...learned, virtual_display_layout: 'exclusive', virtual_display_layout_set_by_user: true };
  assert.deepEqual(changes(learned, picked), {
    virtual_display_layout: 'exclusive',
    virtual_display_layout_set_by_user: true,
  });
  const mine = toDraft(client({ virtual_display_layout: 'exclusive', virtual_display_layout_set_by_user: true }));
  assert.deepEqual(changes(mine, { ...mine, virtual_display_layout: '' }), {
    virtual_display_layout: null,
    virtual_display_layout_set_by_user: null,
  });
});
