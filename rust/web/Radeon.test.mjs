import assert from 'node:assert/strict';
import { readFileSync } from 'node:fs';
import { test } from 'node:test';
import { createTestServer } from './test-server.mjs';

const server = await createTestServer();
const radeon = await server.ssrLoadModule('/src/components/library/radeon.ts');
const table = JSON.parse(readFileSync(new URL('./src/lib/radeon_profiles.json', import.meta.url), 'utf8'));

const setting = (id, extra = {}) => ({ id, group: id.split('.')[0], kind: 'toggle', state: 'configurable', ...extra });
const integer = (id, value, min, max) => setting(id, { kind: 'integer', value, range: { min, max, step: 1 }, unit: 'fps' });
const choice = (id, values) =>
  setting(id, { kind: 'choice', value: values[0], options: values.map(value => ({ value, label: value })) });
/** A GPU that supports every control, as `GET /api/radeon` reports it. */
const everything = [
  setting('anti_lag.enabled'),
  setting('frame_rate_target.enabled'),
  integer('frame_rate_target.fps', 144, 30, 300),
  setting('chill.enabled'),
  integer('chill.min_fps', 60, 30, 300),
  integer('chill.max_fps', 144, 30, 300),
  setting('image_sharpening.enabled'),
  integer('image_sharpening.sharpness', 80, 10, 100),
  setting('anisotropic_filtering.enabled'),
  choice('anti_aliasing.mode', ['use_app_settings', 'enhance_app_settings', 'override_app_settings']),
  choice('anti_aliasing.level', ['invalid', '2x', '4x', '8x']),
  choice('anti_aliasing.method', ['multisampling', 'supersampling']),
  setting('boost.enabled'),
  integer('boost.min_resolution', 50, 50, 100),
  setting('afmf.enabled'),
  choice('vsync.mode', ['always_off', 'always_on']),
];
const panel = settings => ({ status: true, state: 'configurable', gpus: [{ id: 1, name: 'GPU', driver: 'd', settings }] });
const full = radeon.supportOf(panel(everything));
const rows = settings => radeon.rowsOf(settings);
const settingsOf = choice => radeon.settingsOf(choice.rows);

test('every preset round-trips with radeon_profiles.json', () => {
  for (const preset of table.profiles.filter(profile => profile.id !== 'custom')) {
    const settings = Object.fromEntries(preset.settings.map(({ id, value }) => [id, value]));
    assert.deepEqual(radeon.presetSettings(preset.id), settings, preset.id);
    const { controls, used } = radeon.controlsOf(settings, full);
    assert.deepEqual([...used].sort(), Object.keys(settings).sort(), `${preset.id}: the controls show all of it`);
    assert.deepEqual(radeon.settingsOfControls(controls, full), settings, preset.id);
    assert.equal(radeon.profileFor(settings), preset.id);
    // Picking it shows its values in the controls and nothing raw.
    assert.deepEqual(radeon.rawRows(preset.id, [], full), [], preset.id);
  }
  assert.equal(radeon.profileFor({}), '');
  assert.equal(radeon.controlsOf(radeon.presetSettings('smoothness'), full).controls.frameCap, 'stream');
  assert.equal(radeon.controlsOf(radeon.presetSettings('quality'), full).controls.antiAliasing, 'enhance');
});

test('a changed control gives Custom with the preset values plus the change', () => {
  const { controls } = radeon.controlsOf(radeon.presetSettings('competitive'), full);
  const next = radeon.changeControls('competitive', [], full, { ...controls, frameCap: 'fps', fps: 120 });
  assert.equal(next.profile, 'custom');
  assert.deepEqual(settingsOf(next), {
    'anti_lag.enabled': true,
    'frame_rate_target.enabled': true,
    'frame_rate_target.fps': 120,
  });
  // Undoing the change is the preset again, and turning everything off is Off.
  const back = radeon.changeControls('custom', next.rows, full, controls);
  assert.equal(back.profile, 'competitive');
  assert.equal(radeon.changeControls('competitive', [], full, radeon.NO_CONTROLS).profile, '');
  // Controls that add up to another preset pick it.
  const smooth = radeon.changeControls('', [], full, { ...radeon.NO_CONTROLS, frameCap: 'stream' });
  assert.equal(smooth.profile, 'smoothness');
});

test('every control on is the full list, at most MAX_SETTINGS, and the host takes it', () => {
  const all = {
    antiLag: true,
    frameCap: 'fps',
    fps: 90,
    chill: true,
    chillMin: 40,
    chillMax: 90,
    sharpening: true,
    sharpness: 60,
    anisotropic: true,
    antiAliasing: 'override',
    aaLevel: '4x',
    aaMethod: 'supersampling',
    boost: true,
    boostResolution: 70,
  };
  const next = radeon.changeControls('quality', [], full, all);
  assert.equal(next.profile, 'custom');
  const settings = settingsOf(next);
  assert.equal(Object.keys(settings).length, 14);
  assert.ok(Object.keys(settings).length <= radeon.MAX_SETTINGS);
  assert.equal(radeon.profileError(next.profile, next.rows), '');
  // It reads back as the same controls with nothing left raw.
  const { controls, used } = radeon.controlsOf(settings, full);
  assert.deepEqual(controls, all);
  assert.equal(used.size, 14);
  // Level and method are written only under Override.
  const enhanced = settingsOf(radeon.changeControls('custom', next.rows, full, { ...all, antiAliasing: 'enhance' }));
  assert.equal(enhanced['anti_aliasing.mode'], 'enhance_app_settings');
  assert.ok(!('anti_aliasing.level' in enhanced) && !('anti_aliasing.method' in enhanced));
});

test('unsupported settings are hidden and never written by a control', () => {
  const some = radeon.supportOf(
    panel([
      setting('anti_lag.enabled'),
      setting('boost.enabled', { state: 'unsupported' }),
      setting('chill.enabled', { state: 'read_only' }),
      setting('frame_rate_target.enabled'),
      // The cap needs its number too.
    ]),
  );
  assert.deepEqual([...radeon.offered(some)], ['antiLag']);
  const next = radeon.changeControls('', [], some, { ...radeon.NO_CONTROLS, antiLag: true, boost: true, chill: true });
  assert.deepEqual(settingsOf(next), { 'anti_lag.enabled': true });
  // A stored setting of a hidden control is a raw row, not lost.
  const stored = rows({ 'anti_lag.enabled': true, 'boost.enabled': true });
  assert.deepEqual(
    radeon.rawRows('custom', stored, some).map(row => row.id),
    ['boost.enabled'],
  );
  // Without support data there are no controls: everything is raw, even a preset's.
  const none = radeon.supportOf(null);
  assert.equal(radeon.offered(none).size, 0);
  assert.equal(radeon.offered(radeon.supportOf({ status: true, state: 'unsupported', gpus: [] })).size, 0);
  assert.deepEqual(radeon.rawRows('competitive', [], none).map(row => [row.id, row.value]), [['anti_lag.enabled', 'true']]);
});

test('unknown raw ids are kept through control changes', () => {
  const stored = rows({
    'anti_lag.enabled': true,
    'vendor.future_thing': 'on',
    'afmf.enabled': true,
    'chill.enabled': false,
  });
  assert.deepEqual(
    radeon.rawRows('custom', stored, full).map(row => row.id),
    ['vendor.future_thing', 'afmf.enabled', 'chill.enabled'],
  );
  const { controls } = radeon.controlsOf(radeon.settingsOf(stored), full);
  assert.equal(controls.antiLag, true);
  assert.equal(controls.chill, false);
  const next = radeon.changeControls('custom', stored, full, { ...controls, anisotropic: true });
  assert.equal(next.profile, 'custom');
  assert.deepEqual(settingsOf(next), {
    'anti_lag.enabled': true,
    'anisotropic_filtering.enabled': true,
    'vendor.future_thing': 'on',
    'afmf.enabled': true,
    'chill.enabled': false,
  });
  // The raw rows are the same rows, so what is typed in them stays in place.
  assert.ok(next.rows.includes(stored[1]));
  // Turning every control off keeps the raw rows, as Custom.
  const off = radeon.changeControls('custom', next.rows, full, radeon.NO_CONTROLS);
  assert.equal(off.profile, 'custom');
  assert.deepEqual(Object.keys(settingsOf(off)), ['vendor.future_thing', 'afmf.enabled', 'chill.enabled']);
});

test('a value a control cannot show stays raw until the control sets it', () => {
  const stored = rows({ 'chill.enabled': true, 'chill.max_fps': '$stream_fps' });
  const { controls, used } = radeon.controlsOf(radeon.settingsOf(stored), full);
  assert.equal(controls.chill, true);
  assert.equal(controls.chillMax, null);
  assert.deepEqual([...used], ['chill.enabled']);
  assert.deepEqual(radeon.rawRows('custom', stored, full).map(row => row.id), ['chill.max_fps']);
  const next = radeon.changeControls('custom', stored, full, { ...controls, chillMax: 100 });
  assert.deepEqual(settingsOf(next), { 'chill.enabled': true, 'chill.max_fps': 100 });
});

test('picking Custom after a preset starts from its settings, and Add setting does too', () => {
  const picked = radeon.pickProfile('smoothness', [], 'custom');
  assert.deepEqual(settingsOf(picked), radeon.presetSettings('smoothness'));
  // Rows typed earlier are not replaced.
  const typed = rows({ 'a.b': 1 });
  assert.equal(radeon.pickProfile('quality', typed, 'custom').rows, typed);
  const added = radeon.addRow('competitive', []);
  assert.equal(added.profile, 'custom');
  assert.deepEqual(added.rows.map(row => row.id), ['anti_lag.enabled', '']);
});

test('typed numbers are whole and within the reported range', () => {
  const fps = integer('frame_rate_target.fps', 144, 30, 300);
  assert.equal(radeon.clampTo(fps, '999'), 300);
  assert.equal(radeon.clampTo(fps, '10'), 30);
  assert.equal(radeon.clampTo(fps, '89.6'), 90);
  assert.equal(radeon.clampTo(fps, ''), null);
  assert.equal(radeon.clampTo(fps, 'x'), null);
  assert.equal(radeon.currentOf(fps), 144);
  assert.equal(radeon.currentOf(setting('chill.enabled')), null);
});
