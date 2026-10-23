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
const { computeChanges, currentValues, describeDefault, differsFromDefault, isSet, normalise, toText } =
  await server.ssrLoadModule('/src/components/settings/values.ts');
const settings = new Map(schema.settings.map((setting) => [setting.key, setting]));
const table = JSON.parse(readFileSync(new URL('../core/src/stream_presets.json', import.meta.url), 'utf8'));
const explicit = new Set(presets.covered);

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
  assert.deepEqual(computeChanges(shown(stored, edits).byKey, edits, stored, explicit), { stream_preset: 'reference' });
  // Choosing Automatic frame sizes under Reference differs from the preset, so it is saved.
  const both = { stream_preset: 'reference', amd_max_frame_size: 'auto' };
  assert.deepEqual(computeChanges(shown(stored, both).byKey, both, stored, explicit), both);
  // A covered setting the user edits is stored even when it equals the preset's value, so
  // choosing another preset later cannot take it over.
  const same = { stream_preset: 'reference', amd_max_frame_size: '0' };
  assert.deepEqual(computeChanges(shown(stored, same).byKey, same, stored, explicit), same);
  // Unless it is already stored with that value.
  const kept = { stream_preset: 'reference', amd_max_frame_size: '0' };
  assert.deepEqual(computeChanges(shown(kept, same).byKey, same, kept, explicit), {});
  // The rate control is not covered: choosing a mode is an ordinary change.
  const rate = { stream_preset: 'reference', amd_rc: 'cbr' };
  assert.deepEqual(computeChanges(shown(stored, rate).byKey, rate, stored, explicit), rate);
  assert.equal(presets.covered.includes('amd_rc'), false);
  // Clearing the preset removes the name; the hand-set value is untouched.
  const saved = { stream_preset: 'reference', amd_max_frame_size: '4' };
  const cleared = { stream_preset: null };
  assert.deepEqual(computeChanges(shown(saved, cleared).byKey, cleared, saved, explicit), { stream_preset: null });
});

test('a stored value that equals the preset still counts as set by hand', () => {
  const stored = { stream_preset: 'reference', amd_max_frame_size: '0' };
  const { byKey } = shown(stored);
  const frame = byKey.get('amd_max_frame_size');
  assert.equal(differsFromDefault(frame, {}, stored), false);
  assert.equal(isSet(frame, {}, stored), true);
  assert.equal(isSet(byKey.get('amd_quality'), {}, stored), false);
  // An edit that resets it, or one that has not been saved yet, is judged by what the host will have.
  assert.equal(isSet(frame, { amd_max_frame_size: null }, stored), false);
  assert.equal(isSet(frame, { amd_max_frame_size: '0' }, { stream_preset: 'reference' }), true);
});

test('the console reads preset names as the host does, quotes included', () => {
  for (const [text, id] of [
    ['reference', 'reference'],
    ['"Reference"', 'reference'],
    ['  "Reference"  ', 'reference'],
    ['""reference""', 'reference'],
    ['"reference', 'reference'],
    ['" lowest latency "', 'lowest_latency'],
    ['Lowest-Latency', 'lowest_latency'],
    ['"BALANCED"', 'balanced'],
    ['fastest', 'balanced'],
    ['"fastest"', 'balanced'],
    ['', 'balanced'],
  ]) {
    assert.equal(presets.presetId(text), id, text);
  }
  assert.equal(presets.presetId(undefined), 'balanced');
  assert.equal(presets.presetId(3), 'balanced');
  for (const text of ['"Reference"', '""reference""', '" lowest latency "', 'balanced']) {
    assert.equal(presets.presetNotice(text), null, text);
  }
});

test('a stored name the host cannot read is shown as an error, not as Balanced', () => {
  for (const [value, shownAs] of [['fastest', 'fastest'], ['"fastest"', '"fastest"'], [3, '3'], [true, 'true']]) {
    const notice = presets.presetNotice(value);
    assert.match(notice, new RegExp(`^Saved stream preset ${shownAs} is not one of Lowest latency, Balanced, Reference quality`), String(value));
    assert.match(notice, /uses Balanced/);
  }
  for (const value of [undefined, null, '', '   ', '""']) assert.equal(presets.presetNotice(value), null, String(value));
});
