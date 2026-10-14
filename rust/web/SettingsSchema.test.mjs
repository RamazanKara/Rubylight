import assert from 'node:assert/strict';
import { after, test } from 'node:test';
import { createServer } from 'vite';

const server = await createServer({
  configFile: false,
  server: { middlewareMode: true, hmr: false }, appType: 'custom',
  optimizeDeps: { noDiscovery: true, include: [] },
});
after(() => server.close());
const { schema } = await server.ssrLoadModule('/src/lib/settings-schema.ts');
const types = await server.ssrLoadModule('/src/lib/settings-types.ts');
const values =await server.ssrLoadModule('/src/components/settings/values.ts');
const { computeChanges, describeDefault, differsFromDefault, effective, numberError, toServerCommands } = values;
const { matchesSetting, searchWords } = await server.ssrLoadModule('/src/components/settings/search.ts');
const settings = new Map(schema.settings.map(setting => [setting.key, setting]));
const categoryOf = setting => schema.categories.find(category => category.id === setting.category);

test('the default is shown only when the saved value would differ from it', () => {
  const fec = settings.get('fec_percentage');
  assert.equal(differsFromDefault(fec, {}, {}), false);
  assert.equal(differsFromDefault(fec, {}, { fec_percentage: 20 }), false);
  assert.equal(differsFromDefault(fec, {}, { fec_percentage: '"20"' }), false);
  assert.equal(differsFromDefault(fec, {}, { fec_percentage: 30 }), true);
  assert.equal(differsFromDefault(fec, { fec_percentage: 30 }, {}), true);
  // Clearing the field or resetting goes back to the default.
  assert.equal(differsFromDefault(fec, { fec_percentage: '' }, { fec_percentage: 30 }), false);
  assert.equal(differsFromDefault(fec, { fec_percentage: null }, { fec_percentage: 30 }), false);
  // Spellings the host reads as the default.
  assert.equal(differsFromDefault(settings.get('stream_audio'), {}, { stream_audio: 'enabled' }), false);
  assert.equal(differsFromDefault(settings.get('hevc_mode'), {}, { hevc_mode: '0' }), false);
  assert.equal(differsFromDefault(settings.get('hevc_mode'), {}, { hevc_mode: 3 }), true);
  // Zero permanent displays is a real choice; an empty value is the default.
  const permanent = settings.get('dd_virtual_display_permanent_count');
  assert.equal(differsFromDefault(permanent, {}, { dd_virtual_display_permanent_count: 0 }), true);
  assert.equal(differsFromDefault(permanent, {}, {}), false);
});

test('an unset virtual display mode keeps the OS-dependent default without hiding an explicit per-device choice', () => {
  const key = 'virtual_display_mode';
  const setting = settings.get(key);
  assert.equal(effective(setting, {}, {}), '');
  assert.equal(describeDefault(setting).text, 'Automatic (Windows default)');
  assert.deepEqual(computeChanges(settings, { [key]: 'per_client' }, {}), { [key]: 'per_client' });
  assert.deepEqual(computeChanges(settings, { [key]: '' }, { [key]: 'disabled' }), { [key]: null });
});

test('zero permanent displays is a real edit while an empty value leaves the driver unchanged', () => {
  const key = 'dd_virtual_display_permanent_count';
  const setting = settings.get(key);
  assert.equal(effective(setting, {}, {}), '');
  assert.equal(describeDefault(setting).text, 'Not set');
  assert.deepEqual(computeChanges(settings, { [key]: 0 }, {}), { [key]: 0 });
  assert.deepEqual(computeChanges(settings, { [key]: '' }, { [key]: 2 }), { [key]: null });
  assert.deepEqual(computeChanges(settings, { [key]: 0 }, { [key]: '0' }), {});
});

test('library counts and time limits reject fractions instead of saving values the host would ignore', () => {
  // These are Config::integer inputs in core/src/steam.rs and core/src/playnite.rs.
  for (const key of [
    'steam_recent_games', 'steam_recent_max_age_days',
    'playnite_recent_games', 'playnite_recent_max_age_days', 'playnite_autosync_delete_after_days',
    'playnite_focus_attempts', 'playnite_focus_timeout_secs',
  ]) {
    const setting = settings.get(key);
    assert.equal(numberError(setting, '2.5'), 'Enter a whole number.', key);
    assert.equal(numberError(setting, '2'), '', key);
    assert.equal(numberError(setting, '0'), '', key);
    assert.equal(numberError(setting, ''), '', key);
  }
});

test('integer tuning steps reject fractions without forcing multiples of the arrow increment', () => {
  // Both are Config::integer inputs, in host/src/stream.rs and host/src/state.rs.
  for (const key of ['capture_poll_interval_us', 'dd_config_revert_delay']) {
    const setting = settings.get(key);
    assert.equal(numberError(setting, '501.5'), 'Enter a whole number.', key);
    assert.equal(numberError(setting, '501'), '', key);
  }
  for (const [key, value] of [
    ['key_repeat_frequency', '24.9'],
    ['minimum_fps_target', '12.5'],
    ['frame_limiter_fps_limit', '59.94'],
    ['dd_manual_refresh_rate', '119.88'],
  ]) assert.equal(numberError(settings.get(key), value), '', key);
});
