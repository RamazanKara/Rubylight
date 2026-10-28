import assert from 'node:assert/strict';
import { readFileSync } from 'node:fs';
import { test } from 'node:test';
import { createTestServer } from './test-server.mjs';

// The label policy (ux/direction.md 2.5, decided in ux/labels.md): "(Experimental)" stays only on what our
// devices can't verify or whose hardware check failed or is still open. A feature with a hw-queue PASS, a
// measurement or a shipped release without an open bug, and every established-fork port, ships unlabelled.
const server = await createTestServer({ configFile: false });
const { schema } = await server.ssrLoadModule('/src/lib/settings-schema.ts');
const { settings: pending } = await server.ssrLoadModule('/src/lib/schema/extensions.ts');
const { PERMISSION_GROUPS } = await server.ssrLoadModule('/src/components/devices/device.ts');
await server.close();

const all = [...schema.settings, ...pending.filter(item => !schema.settings.some(setting => setting.key === item.key))];
const texts = setting => [
  setting.label,
  setting.description,
  setting.details,
  setting.inactiveHint,
  ...(setting.control?.options ?? []).map(option => option.label),
].filter(text => typeof text === 'string');

const KEPT = [
  '7.1.4 surround (Experimental)',
  'Adapt bitrate on the host (Experimental)',
  'Adaptive packet pacing (Experimental)',
  'Background mode (Experimental)',
  'Client roaming (Experimental)',
  'Device-drawn cursor (Experimental)',
  'Dolby Vision (Experimental)',
  'Encoder focus (Experimental)',
  'Haptics from game audio (Experimental)',
  'Keep monitors off while paused (Experimental)',
  'Pen barrel roll (Experimental)',
  'Rotate the display (Experimental)',
  'Rotation (Experimental)',
  'Send the text around the cursor (Experimental)',
  'Touch (Experimental)',
  'USB forwarding (Experimental)',
  'Windows HDR Calibration on virtual displays (Experimental)',
];

test('the Experimental settings are exactly the policy list, each with the label in its title', () => {
  const labelled = all.filter(setting => texts(setting).some(text => text.includes('Experimental')));
  for (const setting of labelled) assert.match(setting.label, / \(Experimental\)$/, setting.key);
  assert.deepEqual([...new Set(labelled.map(setting => setting.label))].sort(), [...KEPT].sort());
});

test('nothing says (experimental) in lowercase', () => {
  const files = [
    'src/components/devices/DeviceEditor.svelte',
    'src/components/devices/UsbDevices.svelte',
    'src/components/maintenance/ChildSessionPanel.svelte',
    'src/pages/Stream.svelte',
  ].map(file => readFileSync(new URL(file, import.meta.url), 'utf8'));
  const strings = [...all.flatMap(texts), ...PERMISSION_GROUPS.flatMap(group => group.items.map(item => item.label)), ...files];
  for (const text of strings) assert.doesNotMatch(text, /\(experimental\)|experimental\)/, text.slice(0, 80));
});

test('dropped labels stay dropped: established, measured and shipped features', () => {
  for (const key of ['webrtc_server', 'file_transfer', 'shared_folders', 'clipboard_image', 'keyboard_follows_focus',
    'keyboard_layout', 'experimental_packed_444', 'audio_passthrough']) {
    const setting = all.find(item => item.key === key);
    assert(setting, key);
    assert.doesNotMatch(texts(setting).join(' '), /Experimental/, key);
  }
  for (const item of PERMISSION_GROUPS.flatMap(group => group.items)) assert.doesNotMatch(item.label, /Experimental/);
});

test('no fork or credit names in setting text; Moonlight alone is the client', () => {
  for (const setting of all) {
    for (const text of texts(setting)) {
      assert.doesNotMatch(text, /Sunshine|Apollo|Vibepollo|Artemis|Foundation|Moonlight V\+/, `${setting.key}: ${text}`);
    }
  }
});
