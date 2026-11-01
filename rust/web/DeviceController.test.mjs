import assert from 'node:assert/strict';
import { test } from 'node:test';
import { createTestServer } from './test-server.mjs';

globalThis.location = new URL('http://localhost/');
globalThis.window = new EventTarget();
const server = await createTestServer();
const { toDraft, changes, GAMEPAD_OPTIONS } = await server.ssrLoadModule('/src/components/devices/device.ts');
const { render } = await server.ssrLoadModule('svelte/server');
const DeviceEditor = (await server.ssrLoadModule('/src/components/devices/DeviceEditor.svelte')).default;
const client = extra => ({ uuid: 'u', name: 'Switch', perm: 0x071f1f00, enabled: true, connected: false, ...extra });

// Issue #68: Moonlight on a Switch reports no controller type, so Automatic gives it a DualSense
// or Xbox pad, and the host-wide type changed every other device's pad too.
test('a device sets its own controller type, beside its other overrides', () => {
  const before = toDraft(client({ config_overrides: { max_bitrate: 20000 } }));
  assert.equal(before.gamepad, '');
  assert.equal(GAMEPAD_OPTIONS.some(option => option.value === 'vhf_switch'), true);
  assert.deepEqual(changes(before, { ...before, gamepad: 'vhf_switch' }), {
    config_overrides: { max_bitrate: 20000, gamepad: 'vhf_switch' },
  });
  // Automatic is a choice of its own: a device can undo a host-wide type.
  assert.deepEqual(changes(before, { ...before, gamepad: 'auto' }), {
    config_overrides: { max_bitrate: 20000, gamepad: 'auto' },
  });
});

test('a stored type is read back, another host\'s name included, and the host setting removes it', () => {
  const before = toDraft(client({ config_overrides: { gamepad: 'ds5' } }));
  assert.equal(before.gamepad, 'vhf_ds5');
  assert.equal('gamepad' in before.other_overrides, false);
  // Untouched, it goes back as stored.
  assert.deepEqual(changes(before, { ...before, name: 'Lite' }), { name: 'Lite' });
  assert.deepEqual(changes(before, { ...before, gamepad: '' }), { config_overrides: null });
});

test('the editor shows the controller type and opens on one it does not know', () => {
  const body = props => render(DeviceEditor, { props: { client: client(props), sleepUnavailable: null } }).body;
  const section = html => /<details class="override[^>]*><summary[^>]*>Controller</.exec(html)[0];
  assert.match(body({}), /Controller type/);
  assert.doesNotMatch(section(body({ config_overrides: { gamepad: 'vhf_switch' } })), /\sopen/);
  assert.match(section(body({ config_overrides: { gamepad: 'joycon' } })), /\sopen/);
});
