import assert from 'node:assert/strict';
import { readFileSync } from 'node:fs';
import { test } from 'node:test';
import { createTestServer } from './test-server.mjs';

globalThis.location = new URL('http://localhost/');
globalThis.window = new EventTarget();
const server = await createTestServer();
const { schema } = await server.ssrLoadModule('/src/lib/settings-schema.ts');
const { default: SettingControl } = await server.ssrLoadModule('/src/components/settings/SettingControl.svelte');
const { render } = await server.ssrLoadModule('svelte/server');
const presets = await server.ssrLoadModule('/src/lib/stream-presets.ts');
const { computeChanges, currentValues, describeDefault, differsFromDefault, normalise, toText } =
  await server.ssrLoadModule('/src/components/settings/values.ts');
const settings = new Map(schema.settings.map((setting) => [setting.key, setting]));
const table = JSON.parse(readFileSync(new URL('../core/src/stream_presets.json', import.meta.url), 'utf8'));

/** The settings as Settings.svelte shows them under the preset the stored and edited values name. */
function shown(stored = {}, edits = {}) {
  const id = presets.presetId(Object.hasOwn(edits, 'stream_preset') ? edits.stream_preset : stored.stream_preset);
  const described = presets.withPreset(schema.settings, id);
  return { id, described, byKey: new Map(described.map((setting) => [setting.key, setting])) };
}

test('the selector offers the three presets of the host table, Balanced by default', () => {
  const setting = settings.get('stream_preset');
  assert.deepEqual(setting.control.options.map((option) => option.value), ['lowest_latency', 'balanced', 'reference']);
  assert.deepEqual(setting.control.options.map((option) => option.label), ['Lowest latency', 'Balanced', 'Reference quality']);
  assert.equal(setting.default, 'balanced');
  assert.equal(setting.advanced, false);
  const html = render(SettingControl, {
    props: { setting, id: 'stream_preset', value: normalise(setting, 'reference'), onchange: () => {} },
  }).body;
  assert.match(html, /<option[^>]*value="reference"[^>]*selected/);
  assert.match(html, />Lowest latency<\/option>/);
  assert.equal(describeDefault(setting).text, 'Balanced');
});

test('the schema defaults of the covered settings are the host defaults the table lists', () => {
  assert.equal(table.default, 'balanced');
  for (const { key, default: text } of table.covered) {
    const setting = settings.get(key);
    assert(setting, key);
    assert.equal(toText(setting.default), text, `${key} default`);
  }
  // Balanced sets nothing, so it is exactly the defaults.
  assert.deepEqual(presets.presetById('balanced').settings, {});
  assert.equal(shown().described.every((setting, index) => setting === schema.settings[index]), true);
});

test('every value a preset sets is one of the setting\'s options and is covered', () => {
  for (const preset of table.presets) {
    for (const [key, value] of Object.entries(preset.settings)) {
      assert(table.covered.some((entry) => entry.key === key), `${preset.id} ${key} is not covered`);
      const { control } = settings.get(key);
      if (control.kind === 'select') {
        assert(control.options.some((option) => option.value === value), `${preset.id} ${key}=${value}`);
      }
    }
  }
  assert.deepEqual([...presets.covered].sort(), table.covered.map((entry) => entry.key).sort());
});

test('under a preset the covered settings show its values as their defaults', () => {
  const lowest = shown({ stream_preset: 'lowest_latency' });
  assert.equal(lowest.byKey.get('amd_max_frame_size').default, '1');
  assert.equal(lowest.byKey.get('amd_rc').default, 'vbr_latency');
  const reference = shown({ stream_preset: 'reference' });
  // No preset changes the rate control.
  assert.equal(reference.byKey.get('amd_rc').default, 'vbr_latency');
  assert.equal(reference.byKey.get('amd_max_frame_size').default, '0');
  assert.equal(reference.byKey.get('amd_quality').default, 'speed');
  const values = currentValues(reference.described, {}, { stream_preset: 'reference' });
  assert.equal(values.amd_rc, 'vbr_latency');
  assert.equal(values.amd_max_frame_size, '0');
  // Choosing the preset in the console changes the defaults before it is saved.
  const edited = shown({}, { stream_preset: 'reference' });
  assert.equal(currentValues(edited.described, { stream_preset: 'reference' }, {}).amd_max_frame_size, '0');
  // A name the host cannot read means Balanced, as it does there.
  assert.equal(shown({ stream_preset: 'fastest' }).id, 'balanced');
  assert.equal(shown({ stream_preset: ' "Reference" ' }).id, 'reference');
  assert.equal(shown({ stream_preset: 'lowest-latency' }).id, 'lowest_latency');
});

test('a value set by hand is kept and shows as differing from the preset', () => {
  const { byKey } = shown({ stream_preset: 'reference', amd_max_frame_size: '4' });
  const frame = byKey.get('amd_max_frame_size');
  const quality = byKey.get('amd_quality');
  const stored = { stream_preset: 'reference', amd_max_frame_size: '4' };
  assert.equal(differsFromDefault(frame, {}, stored), true);
  assert.equal(differsFromDefault(quality, {}, stored), false);
  // Back to the preset's own value is no longer a manual setting; Automatic now is.
  assert.equal(differsFromDefault(frame, { amd_max_frame_size: '0' }, stored), false);
  assert.equal(differsFromDefault(frame, { amd_max_frame_size: 'auto' }, stored), true);
});

test('saving a preset sends its name only, and a hand-set value stays in the file', () => {
  const stored = {};
  const edits = { stream_preset: 'reference' };
  assert.deepEqual(computeChanges(shown(stored, edits).byKey, edits, stored), { stream_preset: 'reference' });
  // Choosing Automatic frame sizes under Reference differs from the preset, so it is saved.
  const both = { stream_preset: 'reference', amd_max_frame_size: 'auto' };
  assert.deepEqual(computeChanges(shown(stored, both).byKey, both, stored), both);
  // Choosing the preset's own value is not a change.
  const same = { stream_preset: 'reference', amd_max_frame_size: '0' };
  assert.deepEqual(computeChanges(shown(stored, same).byKey, same, stored), { stream_preset: 'reference' });
  // The rate control is not covered: choosing a mode is an ordinary change.
  const rate = { stream_preset: 'reference', amd_rc: 'cbr' };
  assert.deepEqual(computeChanges(shown(stored, rate).byKey, rate, stored), rate);
  assert.equal(presets.covered.includes('amd_rc'), false);
  // Clearing the preset removes the name; the hand-set value is untouched.
  const saved = { stream_preset: 'reference', amd_max_frame_size: '4' };
  const cleared = { stream_preset: null };
  assert.deepEqual(computeChanges(shown(saved, cleared).byKey, cleared, saved), { stream_preset: null });
});
