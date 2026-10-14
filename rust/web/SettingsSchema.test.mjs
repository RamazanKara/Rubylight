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

test('descriptions stay to two short sentences and keep the finer points behind More', () => {
  for (const setting of schema.settings) {
    assert(setting.description.length <= 180, `${setting.key}: ${setting.description.length} characters`);
    assert((setting.description.match(/[.!?](\s|$)/g) ?? []).length <= 2, setting.key);
    if (setting.details !== undefined) assert(setting.details.trim().length > 20, setting.key);
  }
  for (const category of schema.categories) assert(category.description.length <= 48, category.id);
  // Moved, not dropped: the measured PyroWave rates are still there.
  assert.match(settings.get('pyrowave').details, /277 Mbps for 720p60, 399 Mbps for 1080p60 or 1593 Mbps for 4K60/);
});

test('search finds words that moved behind More, and configuration keys', () => {
  const find = (query, key) => matchesSetting(settings.get(key), categoryOf(settings.get(key)), searchWords(query));
  assert.equal(find('1593 Mbps', 'pyrowave'), true);
  assert.equal(find('usbip-win2', 'gamepad'), true);
  assert.equal(find('fec_percentage', 'fec_percentage'), true);
  assert.equal(find('fec percentage', 'fec_percentage'), true);
  assert.equal(find('amd quality', 'nvenc_enable_min_qp'), false);
});

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

test('NVIDIA driver preferences stay visible whatever encoder streams', () => {
  // limiter.rs applies the first two on every stream with an NVIDIA driver;
  // gpu_priority.rs checks the capture GPU, not the encoder.
  for (const key of ['nvenc_realtime_hags', 'nvenc_latency_over_power', 'nvenc_opengl_vulkan_on_dxgi']) {
    const setting = settings.get(key);
    for (const encoder of ['auto', 'amf', 'quicksync', 'software', 'nvenc']) {
      assert.equal(setting.visibleWhen?.({ encoder }) ?? true, true, `${key} with ${encoder}`);
    }
    assert.doesNotMatch(`${setting.inactiveHint ?? ''} ${setting.group}`, /NVENC/, key);
  }
});

test('Automatic shows the encoder settings for the GPUs this PC has', () => {
  const { NATIVE_ENCODERS } = types;
  const gpu = (name, vendor) => ({ name, vendor, dedicated_memory: 0, luid: [1, 0], pnp_id: null });
  const radeon = gpu('AMD Radeon RX 7900 XT', 0x1002), geforce = gpu('NVIDIA GeForce RTX 4090', 0x10de);
  // As encoder.rs native_backend: by vendor, else by name; Intel has no native encoder.
  assert.deepEqual(values.nativeEncoders([radeon, gpu('Intel(R) Arc(TM) A770', 0x8086)], ''), ['amf']);
  assert.deepEqual(values.nativeEncoders([gpu('AMD Radeon R7 M370', 0)], ''), ['amf']);
  assert.deepEqual(values.nativeEncoders([radeon, geforce], ''), ['amf', 'nvenc']);
  assert.deepEqual(values.nativeEncoders([radeon, geforce], 'nvidia geforce rtx 4090'), ['nvenc']);
  assert.deepEqual(values.nativeEncoders(null, ''), []);
  const amd = settings.get('amd_usage'), nvidia = settings.get('nvenc_preset');
  assert.equal(amd.visibleWhen({ encoder: 'auto', [NATIVE_ENCODERS]: ['amf'] }), true);
  assert.equal(amd.visibleWhen({ encoder: 'auto', [NATIVE_ENCODERS]: ['nvenc'] }), false);
  assert.equal(amd.visibleWhen({ encoder: 'nvenc', [NATIVE_ENCODERS]: ['amf'] }), false);
  assert.equal(nvidia.visibleWhen({ encoder: 'pyrowave', [NATIVE_ENCODERS]: ['nvenc'] }), true);
  assert.equal(nvidia.visibleWhen({ encoder: 'auto' }), false);
  assert.equal(settings.get('amd_qvbr_quality_level').visibleWhen({ encoder: 'auto', amd_rc: 'qvbr', [NATIVE_ENCODERS]: ['amf'] }), true);
});

test('frame rates the host parses reject a fourth decimal place', () => {
  // framegen.rs Rate::parse: a fixed refresh rate with four decimals fails every stream.
  for (const key of ['dd_manual_refresh_rate', 'frame_limiter_fps_limit']) {
    const setting = settings.get(key);
    assert.equal(numberError(setting, '59.9401'), 'Enter at most three decimal places.', key);
    assert.equal(numberError(setting, '59.94'), '', key);
    assert.equal(numberError(setting, '119.880'), '', key);
    assert.equal(numberError(setting, '120'), '', key);
  }
  assert.equal(numberError(settings.get('key_repeat_frequency'), '24.95'), '');
});

test('a fixed resolution must be WIDTHxHEIGHT within the host limits', () => {
  assert.equal(typeof values.textError, 'function');
  const setting = settings.get('dd_manual_resolution');
  for (const ok of ['', '2560x1440', ' 1920 x 1080 ', '320x200', '7680x4320']) assert.equal(values.textError(setting, ok), '', ok);
  for (const bad of ['2560*1440', '2560X1440', '2560x', 'x1440', '2560x1440x60']) {
    assert.equal(values.textError(setting, bad), 'Enter WIDTHxHEIGHT, for example 2560x1440.', bad);
  }
  for (const bad of ['319x200', '7681x1080', '1920x199', '1920x4321']) {
    assert.match(values.textError(setting, bad), /width from 320 to 7680/, bad);
  }
  assert.equal(values.textError(settings.get('fallback_mode'), 'anything'), '');
  assert.equal(setting.requiredWhen({ dd_resolution_option: 'manual' }), true);
  assert.equal(setting.requiredWhen({ dd_resolution_option: 'auto' }), false);
  assert.equal(settings.get('dd_manual_refresh_rate').requiredWhen({ dd_refresh_rate_option: 'manual' }), true);
});

test('editing commands keeps fields the console does not show', () => {
  const prep = { do: 'a.cmd', undo: 'b.cmd', elevated: false, timeout_ms: 500 };
  const changes = computeChanges(settings, { global_prep_cmd: [{ ...prep, do: ' c.cmd ' }] }, { global_prep_cmd: [prep] });
  assert.deepEqual(changes, { global_prep_cmd: [{ ...prep, do: 'c.cmd' }] });
  const host = { name: 'Lock', cmd: 'lock.cmd', elevated: false, icon: 'lock' };
  assert.deepEqual(computeChanges(settings, { server_cmd: [{ ...host, name: 'Lock PC' }] }, { server_cmd: [host] }), {
    server_cmd: [{ ...host, name: 'Lock PC' }],
  });
});

test('host commands are shown as elevated only when the host would elevate them', () => {
  // stream.rs reads elevated with as_bool: the text "true" is not elevated.
  assert.equal(toServerCommands([{ name: 'a', cmd: 'a', elevated: 'true' }])[0].elevated, false);
  assert.equal(toServerCommands([{ name: 'a', cmd: 'a', elevated: true }])[0].elevated, true);
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
