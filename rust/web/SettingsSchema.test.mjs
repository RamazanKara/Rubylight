import assert from 'node:assert/strict';
import { test } from 'node:test';
import { createTestServer } from './test-server.mjs';

const server = await createTestServer({ configFile: false });
const { schema } = await server.ssrLoadModule('/src/lib/settings-schema.ts');
const values =await server.ssrLoadModule('/src/components/settings/values.ts');
const { computeChanges, describeDefault, differsFromDefault, effective, numberError, toServerCommands } = values;
const { matchesSetting, searchWords } = await server.ssrLoadModule('/src/components/settings/search.ts');
const settings = new Map(schema.settings.map(setting => [setting.key, setting]));
const categoryOf = setting => schema.categories.find(category => category.id === setting.category);

test('AMF frame caps distinguish automatic, driver default and saved fixed limits', () => {
  const setting = settings.get('amd_max_frame_size');
  assert.equal(setting.default, 'auto');
  assert(setting.control.options.some(option => option.value === 'auto'));
  assert.equal(effective(setting, {}, {}), 'auto');
  assert.equal(differsFromDefault(setting, {}, { amd_max_frame_size: 0 }), true);
  assert.equal(differsFromDefault(setting, {}, { amd_max_frame_size: 2 }), true);
  assert.equal(differsFromDefault(setting, {}, { amd_max_frame_size: 'auto' }), false);
});

test('disconnect restoration defaults on and the exclusive-layout delay stays available', () => {
  assert.equal(settings.get('dd_config_revert_on_disconnect').default, true);
  assert.equal(settings.get('dd_config_revert_delay').visibleWhen, undefined);
  const retention = settings.get('dd_paused_virtual_display_timeout_secs');
  assert.equal(retention.visibleWhen({}), false);
  assert.equal(retention.visibleWhen({ dd_config_revert_on_disconnect: false }), true);
});

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
  assert.equal(find('amd quality', 'amd_quality'), true);
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
});

test('spellings the host reads as an option show as that option', () => {
  const shown = (key, stored) => values.normalise(settings.get(key), stored);
  for (const [key, stored, option] of [
    // encoder_policy.rs tristate; the fallback console writes true and false.
    ['amd_vbaq', true, 'enabled'],
    ['amd_vbaq', 'false', 'disabled'],
    ['amd_vbaq', ' ON ', 'enabled'],
    ['amd_vbaq', 0, 'disabled'],
    ['amd_lowlatency_mode', 'true', 'enabled'],
    ['amd_high_motion_quality_boost', false, 'disabled'],
    ['amd_smart_access_video', 'off', 'disabled'],
    ['amd_split_frame', 'Yes', 'enabled'],
    ['amd_av1_screen_content', 1, 'enabled'],
    // framegen.rs reads letters and digits only.
    ['frame_limiter_auto_virtual_framegen', 'fixed-1000hz', 'vrr'],
    ['frame_limiter_auto_virtual_framegen', 'fixed-1000-hz', 'vrr'],
    ['frame_limiter_auto_virtual_framegen', 1000, 'vrr'],
    ['frame_limiter_auto_virtual_framegen', 'fixed_2x', 'legacy'],
    ['frame_limiter_auto_virtual_framegen', 'Smooth', 'enabled'],
    ['frame_limiter_auto_virtual_framegen', false, 'disabled'],
    ['rtss_frame_limit_type', 'front-edge-sync', 'front edge sync'],
    ['rtss_frame_limit_type', 'back_edge_sync', 'back edge sync'],
    ['frame_limiter_provider', 'disabled', 'none'],
    // config.rs log_level; the fallback console writes numbers.
    ['min_log_level', 0, 'verbose'],
    ['min_log_level', 'trace', 'verbose'],
    ['min_log_level', 2, 'info'],
    ['min_log_level', '3', 'warning'],
    ['min_log_level', 'WARN', 'warning'],
    ['min_log_level', 5, 'error'],
    ['min_log_level', 6, 'none'],
    ['min_log_level', 'Debug', 'debug'],
    ['encoder', 'amdvce', 'amf'],
    ['encoder', 'amdvce_experimental', 'amf'],
    ['encoder', 'amdvce_ffmpeg', 'amf'],
    ['encoder', 'amdvce_legacy', 'amf'],
    ['capture', 'dxgi', 'ddx'],
    ['capture', 'WGCC', 'wgc'],
    ['capture', 'DDX', 'ddx'],
    ['frame_pacing', 'fixed', 'grid'],
    ['virtual_display_layout', 'Extended-Primary', 'extended_primary'],
    ['virtual_display_layout', 'ensure_only_display', 'exclusive'],
    ['gamepad', 'ds5', 'vhf_ds5'],
    ['gamepad', 'x360', 'vhf_xbox_one'],
    ['amd_coder', 'vlc', 'cavlc'],
    ['amd_coder', 'ac', 'cabac'],
    ['locale', 'pt-br', 'pt_BR'],
    ['locale', 'DE', 'de'],
  ]) {
    assert.equal(shown(key, stored), option, `${key} = ${JSON.stringify(stored)}`);
  }
  // Where the host compares exactly, or does not read a spelling, it stays as saved.
  for (const [key, stored] of [
    ['encoder', 'AMDVCE'],
    ['amd_coder', 'CABAC'],
    ['gamepad', 'DS5'],
    ['amd_vbaq', 'maybe'],
    ['min_log_level', '7'],
    ['frame_limiter_auto_virtual_framegen', '4x'],
    ['amd_usage', 'ULTRALOWLATENCY'],
  ]) {
    assert.equal(shown(key, stored), stored, `${key} = ${stored}`);
  }
  assert.equal(shown('amd_vbaq', ''), '');
});

test('every alias names one of its setting\'s options', () => {
  for (const setting of schema.settings) {
    if (!setting.aliases) continue;
    assert.equal(setting.control.kind, 'select', setting.key);
    const options = new Set(setting.control.options.map(option => option.value));
    for (const [spelling, option] of Object.entries(setting.aliases.spellings ?? {})) {
      assert(options.has(option), `${setting.key}: ${spelling} -> ${option}`);
      assert(!options.has(spelling), `${setting.key}: ${spelling} is an option`);
    }
  }
});

test('capture-specific controls follow the selected method and remain available for Automatic', () => {
  const predictive = settings.get('capture_predictive_poll');
  const aligned = settings.get('wgc_slot_aligned_publish');
  const fences = settings.get('idd_push_fence_ring');
  assert.equal(fences.default, true);
  for (const [capture, polling, alignment] of [
    ['auto', true, true], ['idd-push', false, false],
    ['wgc', false, true], ['wgcc', false, true],
    ['ddx', true, false], ['dxgi', true, false],
  ]) {
    assert.equal(predictive.visibleWhen({ capture }), polling, capture);
    assert.equal(aligned.visibleWhen({ capture }), alignment, capture);
    assert.equal(fences.visibleWhen({ capture }), ['auto', 'idd-push'].includes(capture), capture);
  }
});

test('a saved alias is written back only when the edit means something else', () => {
  const vbaq = settings.get('amd_vbaq');
  // Choosing the option the alias already means sends nothing.
  assert.deepEqual(computeChanges(settings, { amd_vbaq: 'enabled' }, { amd_vbaq: 'true' }), {});
  assert.deepEqual(computeChanges(settings, { min_log_level: 'info' }, { min_log_level: 2 }), {});
  assert.deepEqual(computeChanges(settings, { amd_vbaq: 'disabled' }, { amd_vbaq: 'true' }), { amd_vbaq: 'disabled' });
  assert.deepEqual(computeChanges(settings, { amd_vbaq: null }, { amd_vbaq: 'true' }), { amd_vbaq: null });
  // An alias of the default is the default.
  assert.equal(differsFromDefault(settings.get('frame_limiter_auto_virtual_framegen'), {}, { frame_limiter_auto_virtual_framegen: '2x' }), false);
  assert.equal(differsFromDefault(settings.get('min_log_level'), {}, { min_log_level: 2 }), false);
  assert.equal(differsFromDefault(vbaq, {}, { amd_vbaq: 'true' }), true);
  // Visibility follows what the host reads.
  const current = values.currentValues(schema.settings, {}, { encoder: 'amdvce', amd_rc: 'qvbr' });
  assert.equal(current.encoder, 'amf');
  assert.equal(settings.get('amd_qvbr_quality_level').visibleWhen(current), true);
});

test('AMD AMF settings stay visible whatever the encoder setting says', () => {
  // AMF encodes every stream except PyroWave's own, so the group is never hidden by the encoder.
  const amd = schema.settings.filter(setting => setting.group === 'AMD AMF');
  assert(amd.length > 10);
  for (const setting of amd) {
    assert.doesNotMatch(setting.inactiveHint ?? '', /Automatic selects|is selected/, setting.key);
    if (setting.key === 'amd_qvbr_quality_level') continue;
    for (const encoder of [undefined, 'auto', 'amf', 'amdvce', 'amdvce_experimental', 'amdvce_ffmpeg', 'amdvce_legacy', 'pyrowave']) {
      assert.equal(setting.visibleWhen?.(encoder === undefined ? {} : { encoder }) ?? true, true, `${setting.key} with ${encoder}`);
    }
  }
  assert.equal(settings.get('amd_qvbr_quality_level').visibleWhen({ encoder: 'pyrowave', amd_rc: 'qvbr' }), true);
  assert.equal(settings.get('amd_qvbr_quality_level').visibleWhen({ encoder: 'amf', amd_rc: 'cbr' }), false);
});

test('no encoder other than AMD AMF and PyroWave is offered', () => {
  assert.deepEqual(settings.get('encoder').control.options.map(option => option.value), ['auto', 'amf', 'pyrowave']);
  assert.deepEqual(Object.keys(settings.get('encoder').aliases.spellings).sort(), ['amdvce', 'amdvce_experimental', 'amdvce_ffmpeg', 'amdvce_legacy']);
  assert.deepEqual(settings.get('frame_limiter_provider').control.options.map(option => option.value), ['auto', 'rtss', 'none']);
  assert.deepEqual(settings.get('rtss_frame_limit_type').control.options.map(option => option.value), ['async', 'front edge sync', 'back edge sync']);
  for (const setting of schema.settings) assert.doesNotMatch(setting.key, /^(nvenc_|qsv_|sw_)|^(min_threads|frame_limiter_disable_vsync|dd_wa_dummy_plug_hdr10)$/, setting.key);
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

test('restore hotkey modifiers hide while the hotkey is off in any spelling the host accepts', () => {
  const setting = settings.get('dd_snapshot_restore_hotkey_modifiers');
  for (const off of ['', '0', 'none', 'OFF', ' disabled ']) {
    assert.equal(setting.visibleWhen({ dd_snapshot_restore_hotkey: off }), false, off);
  }
  assert.equal(setting.visibleWhen({ dd_snapshot_restore_hotkey: 'F12' }), true);
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

test('empty mode substitutions read as none, not a built-in configuration', () => {
  assert.equal(describeDefault(settings.get('dd_mode_remapping')).text, 'None');
});

test('an unset virtual display mode keeps the OS-dependent default without hiding an explicit per-device choice', () => {
  const key = 'virtual_display_mode';
  const setting = settings.get(key);
  assert.equal(effective(setting, {}, {}), '');
  assert.equal(describeDefault(setting).text, 'Default (one for each device)');
  assert.deepEqual(computeChanges(settings, { [key]: 'per_client' }, {}), { [key]: 'per_client' });
  assert.deepEqual(computeChanges(settings, { [key]: '' }, { [key]: 'disabled' }), { [key]: null });
});

test('settings for one client or capture quirk stay out of the basic view', () => {
  // limit_framerate matters only to Artemis Warp, stream_reconfigure only to Rubylight clients.
  for (const key of ['limit_framerate', 'stream_reconfigure']) assert.equal(settings.get(key).advanced, true, key);
  assert.equal(settings.get('stream_preset').advanced, false);
  assert.equal(settings.get('max_bitrate').advanced, false);
});

test('capture quirks sit together in an advanced troubleshooting group and stay available', () => {
  const keys = [
    'capture_poll_interval_us', 'capture_predictive_poll', 'wgc_slot_aligned_publish',
    'wgc_direct_encoder_input', 'gpu_compute_conversion', 'idd_push_fence_ring',
  ];
  for (const key of keys) {
    assert.equal(settings.get(key).group, 'Capture troubleshooting', key);
    assert.equal(settings.get(key).advanced, true, key);
  }
  const grouped = schema.settings.filter(setting => setting.group === 'Capture troubleshooting');
  assert.deepEqual(grouped.map(setting => setting.key).sort(), [...keys].sort());
  assert.equal(settings.get('capture').group, 'Capture');
});

test('bitrate and pacing controllers say which one wins', () => {
  // The finer points live behind More; descriptions are capped at two short sentences.
  const says = (key, pattern) => assert.match(settings.get(key).details, pattern, key);
  says('link_rate_adaptation', /Adapt bitrate on the host wins[^.]*sets its own bitrate takes over/);
  says('host_adaptive_bitrate', /replaces[^.]*sets its own bitrate takes over/);
  says('pacing_max_bitrate_kbps', /turns off Adaptive packet pacing and Slow down after video loss/);
  says('host_adaptive_pacing', /manual pacing rate or Adapt bitrate on the host turns it off/);
  says('keyframe_loss_pacing', /manual pacing rate, Adaptive packet pacing or Adapt bitrate on the host turns it off/);
});

test('the still-picture resend rate is not named like a floor on game frame rate', () => {
  const setting = settings.get('minimum_fps_target');
  assert.equal(setting.label, 'Still-picture resend rate');
  assert.match(setting.description, /unchanged picture/);
});

test('activating a virtual display is offered only where it changes the stream', () => {
  // display_session.rs: a virtual display is configured when the mode is not "disabled" or this is on.
  const activate = settings.get('dd_activate_virtual_display');
  assert.equal(activate.visibleWhen({}), true, 'unset is off on Windows 10');
  assert.equal(activate.visibleWhen({ virtual_display_mode: '' }), true);
  assert.equal(activate.visibleWhen({ virtual_display_mode: 'disabled' }), true);
  assert.equal(activate.visibleWhen({ virtual_display_mode: 'per_client' }), false);
  assert.equal(activate.visibleWhen({ virtual_display_mode: 'shared' }), false);
  assert.ok(activate.inactiveHint);
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
