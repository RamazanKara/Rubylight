import assert from 'node:assert/strict';
import { test } from 'node:test';
import { createTestServer } from './test-server.mjs';

globalThis.location = new URL('http://localhost/');
globalThis.window = new EventTarget();
const server = await createTestServer();
const { toDraft, changes, LTR_FRAME_OPTIONS } = await server.ssrLoadModule('/src/components/devices/device.ts');
const { render } = await server.ssrLoadModule('svelte/server');
const DeviceEditor = (await server.ssrLoadModule('/src/components/devices/DeviceEditor.svelte')).default;
const client = extra => ({ uuid: 'u', name: 'Fire TV', perm: 0x071f1f00, enabled: true, connected: false, ...extra });

// Issue #73: a Moonlight client on a Fire TV Cube freezes on long-term references, and turning them
// off in Settings took them from every other device too.
test('a device sets its own long-term reference frames, beside its other overrides', () => {
  const before = toDraft(client({ config_overrides: { max_bitrate: 20000 } }));
  assert.equal(before.ltr_frames, '');
  assert.deepEqual(LTR_FRAME_OPTIONS.map(option => option.value), ['0', '1', '2', '3', '4']);
  assert.equal(LTR_FRAME_OPTIONS[0].label, 'Off');
  // A number, and 0 is a value of its own rather than "use the host setting".
  assert.deepEqual(changes(before, { ...before, ltr_frames: '0' }), {
    config_overrides: { max_bitrate: 20000, amd_ltr_frames: 0 },
  });
  assert.deepEqual(changes(before, { ...before, ltr_frames: '2' }), {
    config_overrides: { max_bitrate: 20000, amd_ltr_frames: 2 },
  });
});

test('stored frames are read back, and the host setting removes them', () => {
  const before = toDraft(client({ config_overrides: { amd_ltr_frames: 0 } }));
  assert.equal(before.ltr_frames, '0');
  assert.equal('amd_ltr_frames' in before.other_overrides, false);
  assert.deepEqual(changes(before, { ...before, name: 'Cube' }), { name: 'Cube' });
  assert.deepEqual(changes(before, { ...before, ltr_frames: '' }), { config_overrides: null });
});

test('a stored value the console cannot show goes back as stored', () => {
  for (const stored of ['0', 7, true]) {
    const before = toDraft(client({ config_overrides: { amd_ltr_frames: stored, gamepad: 'ds5' } }));
    const after = { ...before, gamepad: '' };
    assert.deepEqual(changes(before, after), { config_overrides: { amd_ltr_frames: stored } });
  }
});

test('the editor shows the frames and opens on a value it does not offer', () => {
  const body = props => render(DeviceEditor, { props: { client: client(props), sleepUnavailable: null } }).body;
  const section = html => /<details class="override[^>]*><summary[^>]*>Encoder overrides</.exec(html)[0];
  assert.match(body({}), /Long-term reference frames/);
  assert.doesNotMatch(section(body({})), /\sopen/);
  assert.doesNotMatch(section(body({ config_overrides: { amd_ltr_frames: 0 } })), /\sopen/);
  assert.match(section(body({ config_overrides: { amd_ltr_frames: 7 } })), /\sopen/);
});
