import assert from 'node:assert/strict';
import { test } from 'node:test';
import { createTestServer } from './test-server.mjs';

const server = await createTestServer({ configFile: false });
const { schema } = await server.ssrLoadModule('/src/lib/settings-schema.ts');
const types = await server.ssrLoadModule('/src/lib/settings-types.ts');
const values =await server.ssrLoadModule('/src/components/settings/values.ts');
const { computeChanges, describeDefault, differsFromDefault, effective, numberError, toServerCommands } = values;
const { matchesSetting, searchWords } = await server.ssrLoadModule('/src/components/settings/search.ts');
const { settings: pending } = await server.ssrLoadModule('/src/lib/schema/extensions.ts');
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

test('HDR dynamic metadata defaults on and ships HDR Vivid as a normal feature', () => {
  const setting = settings.get('hdr_dynamic_metadata');
  assert.equal(setting.default, true);
  assert.equal(setting.control.kind, 'toggle');
  assert.equal(setting.category, 'video');
  assert.match(setting.details, /HEVC streams also carry HDR Vivid/);
  // Stock Moonlight strips HEVC SEI, so the reach is stated, not implied.
  assert.match(setting.details, /Moonlight drops them from HEVC before the decoder; Nightjar keeps them/);
  assert.doesNotMatch(`${setting.label} ${setting.description} ${setting.details}`, /experimental/i);
  assert.equal(differsFromDefault(setting, {}, {}), false);
  assert.equal(differsFromDefault(setting, {}, { hdr_dynamic_metadata: false }), true);
});

test('Dolby Vision is a live setting, without an Experimental label, that says why packed 4:4:4 streams go without it', () => {
  const setting = settings.get('hdr_dolby_vision');
  assert.equal(setting.default, true);
  assert.equal(setting.control.kind, 'toggle');
  assert.equal(setting.category, 'video');
  assert.equal(setting.label, 'Dolby Vision');
  assert.doesNotMatch(`${setting.label} ${setting.description}`, /Experimental/i);
  assert.match(setting.description, /profile 10\.1 on AV1 HDR streams/);
  assert.match(setting.details, /Packed 4:4:4 streams carry static HDR10 instead: the device unpacks the packed picture itself/);
  assert.doesNotMatch(`${setting.label} ${setting.description} ${setting.details}`, /Foundation|dovi_tool|quietvoid/);
  assert.equal(pending.some(candidate => candidate.key === 'hdr_dolby_vision'), false);
  assert.equal(differsFromDefault(setting, {}, { hdr_dolby_vision: false }), true);
});

test('HLG is a live, normal setting that leaves packed 4:4:4 streams on HDR10', () => {
  const setting = settings.get('hdr_hlg');
  assert.equal(setting.default, true);
  assert.equal(setting.control.kind, 'toggle');
  assert.equal(setting.category, 'video');
  assert.equal(setting.label, 'HLG');
  assert.match(setting.details, /Packed 4:4:4 streams stay HDR10/);
  assert.doesNotMatch(`${setting.label} ${setting.description} ${setting.details}`, /experimental|Foundation/i);
  assert.equal(pending.some(candidate => candidate.key === 'hdr_hlg'), false);
  assert.equal(differsFromDefault(setting, {}, { hdr_hlg: false }), true);
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
  assert.equal(find('nvenc preset', 'nvenc_preset'), true);
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
    ['rtss_frame_limit_type', 'nvidia-reflex', 'nvidia reflex'],
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
    ['encoder', 'nvenc_experimental', 'nvenc'],
    ['encoder', 'nvenc_legacy', 'nvenc'],
    // nvenc.rs; Vibepollo saved these as nvenc_force_split_encode.
    ['nvenc_split_encode', 'forced', 'enabled'],
    ['nvenc_split_encode', true, 'enabled'],
    ['nvenc_split_encode', 'false', 'disabled'],
    ['nvenc_split_encode', 'driver_decides', 'auto'],
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
    ['nvenc_split_encode', 'Forced'],
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

test('each encoder group shows for its encoder, and Automatic shows the one for this PC\'s GPUs', () => {
  const { NATIVE_ENCODERS } = types;
  const groups = { amf: 'AMD AMF', nvenc: 'NVIDIA NVENC' };
  const conditional = ['amd_qvbr_quality_level', 'nvenc_target_quality', 'nvenc_peak_bitrate_ratio', 'nvenc_min_qp_h264', 'nvenc_min_qp_hevc', 'nvenc_min_qp_av1'];
  for (const [backend, group] of Object.entries(groups)) {
    const members = schema.settings.filter(setting => setting.group === group);
    assert(members.length > 10, group);
    for (const setting of members) {
      assert.match(setting.inactiveHint ?? '', backend === 'amf' ? /AMD AMF/ : /NVIDIA NVENC/, setting.key);
      if (conditional.includes(setting.key)) continue;
      const shown = (values) => setting.visibleWhen(values);
      // Chosen by name, including Apollo's and Vibepollo's names, whatever the GPUs.
      for (const encoder of backend === 'amf' ? ['amf', 'amdvce', 'amdvce_legacy'] : ['nvenc', 'nvenc_experimental', 'nvenc_legacy']) {
        assert.equal(shown({ encoder, [NATIVE_ENCODERS]: ['amf', 'nvenc'] }), true, `${setting.key} with ${encoder}`);
        assert.equal(shown({ encoder, [NATIVE_ENCODERS]: [] }), true, `${setting.key} with ${encoder}`);
      }
      // Naming the other vendor's encoder hides it.
      assert.equal(shown({ encoder: backend === 'amf' ? 'nvenc' : 'amf', [NATIVE_ENCODERS]: ['amf', 'nvenc'] }), false, setting.key);
      // Automatic (and PyroWave, automatic for other codecs) follows the GPUs.
      for (const encoder of [undefined, 'auto', 'pyrowave']) {
        assert.equal(shown({ encoder, [NATIVE_ENCODERS]: [backend] }), true, `${setting.key} with ${encoder}`);
        assert.equal(shown({ encoder, [NATIVE_ENCODERS]: [backend === 'amf' ? 'nvenc' : 'amf'] }), false, `${setting.key} with ${encoder}`);
        assert.equal(shown({ encoder, [NATIVE_ENCODERS]: ['amf', 'nvenc'] }), true, `${setting.key} with ${encoder}`);
      }
      // Before the GPUs are known, AMF shows as before and NVENC waits.
      assert.equal(shown({ encoder: 'auto' }), backend === 'amf', setting.key);
    }
  }
  const nv = { encoder: 'auto', [NATIVE_ENCODERS]: ['nvenc'] };
  assert.equal(settings.get('amd_qvbr_quality_level').visibleWhen({ encoder: 'pyrowave', amd_rc: 'qvbr' }), true);
  assert.equal(settings.get('amd_qvbr_quality_level').visibleWhen({ encoder: 'amf', amd_rc: 'cbr' }), false);
  assert.equal(settings.get('amd_qvbr_quality_level').visibleWhen({ ...nv, amd_rc: 'qvbr' }), false);
  assert.equal(settings.get('nvenc_target_quality').visibleWhen({ ...nv, nvenc_rate_control: 'vbr' }), true);
  assert.equal(settings.get('nvenc_target_quality').visibleWhen(nv), false);
  assert.equal(settings.get('nvenc_target_quality').visibleWhen({ encoder: 'amf', nvenc_rate_control: 'vbr' }), false);
  assert.equal(settings.get('nvenc_min_qp_hevc').visibleWhen({ ...nv, nvenc_enable_min_qp: 'true' }), true);
  assert.equal(settings.get('nvenc_min_qp_hevc').visibleWhen(nv), false);
});

test('encoder focus shows in the Encoder group for AMF and for NVENC', () => {
  const { NATIVE_ENCODERS } = types;
  const focus = settings.get('encoder_focus');
  assert.equal(focus.group, 'Encoder');
  assert.match(focus.inactiveHint, /AMD AMF or NVIDIA NVENC/);
  for (const encoder of ['amf', 'nvenc']) {
    assert.equal(focus.visibleWhen({ encoder, [NATIVE_ENCODERS]: ['amf', 'nvenc'] }), true, encoder);
  }
  for (const native of [['amf'], ['nvenc']]) {
    assert.equal(focus.visibleWhen({ encoder: 'auto', [NATIVE_ENCODERS]: native }), true, native[0]);
  }
});

test('Automatic uses AMF on AMD and NVENC on NVIDIA GPUs, or on the chosen graphics card', () => {
  const gpu = (name, vendor, pnp_id = null) => ({ name, vendor, dedicated_memory: 0, luid: [1, 0], pnp_id });
  const radeon = gpu('AMD Radeon RX 7900 XT', 0x1002, 'PCI\\VEN_1002&DEV_744C\\1');
  const geforce = gpu('NVIDIA GeForce RTX 4090', 0x10de, 'PCI\\VEN_10DE&DEV_2684\\1');
  // As encoder_gpu.rs: by vendor ID; Intel has no native encoder.
  assert.deepEqual(values.nativeEncoders([radeon, gpu('Intel(R) Arc(TM) A770', 0x8086)], ''), ['amf']);
  assert.deepEqual(values.nativeEncoders([radeon, geforce], ''), ['amf', 'nvenc']);
  assert.deepEqual(values.nativeEncoders([radeon, geforce], 'nvidia geforce rtx 4090'), ['nvenc']);
  // The device ID wins over the name, as on the host.
  assert.deepEqual(values.nativeEncoders([radeon, geforce], 'NVIDIA GeForce RTX 4090', 'pci\\ven_1002&dev_744c\\1'), ['amf']);
  assert.deepEqual(values.nativeEncoders([radeon], 'Unavailable GPU'), []);
  assert.deepEqual(values.nativeEncoders(null, ''), []);
});

test('the encoders offered are Automatic, AMD AMF, NVIDIA NVENC and PyroWave', () => {
  assert.deepEqual(settings.get('encoder').control.options.map(option => option.value), ['auto', 'amf', 'nvenc', 'pyrowave']);
  assert.deepEqual(Object.keys(settings.get('encoder').aliases.spellings).sort(), ['amdvce', 'amdvce_experimental', 'amdvce_ffmpeg', 'amdvce_legacy', 'nvenc_experimental', 'nvenc_legacy']);
  assert.deepEqual(settings.get('frame_limiter_provider').control.options.map(option => option.value), ['auto', 'rtss', 'none']);
  assert.deepEqual(settings.get('rtss_frame_limit_type').control.options.map(option => option.value), ['async', 'front edge sync', 'back edge sync', 'nvidia reflex']);
  for (const setting of schema.settings) assert.doesNotMatch(setting.key, /^(qsv_|sw_)|^(min_threads|frame_limiter_disable_vsync|dd_wa_dummy_plug_hdr10)$/, setting.key);
});

test('the NVENC group holds exactly the settings the NVENC encoder reads, with its defaults', async () => {
  const { readFile } = await import('node:fs/promises');
  // The encoder's settings, the GPU priority it runs at and the driver's power profile.
  const source = (await Promise.all(['../core/src/nvenc.rs', '../windows/src/gpu_priority.rs', '../windows/src/nvidia_profile.rs']
    .map(path => readFile(new URL(path, import.meta.url), 'utf8')))).join('\n');
  const read = [...new Set([...source.matchAll(/"(nvenc_[a-z0-9_]+)"/g)].map(match => match[1]))].sort();
  const shown = schema.settings.filter(setting => setting.key.startsWith('nvenc_'));
  assert.deepEqual(shown.map(setting => setting.key).sort(), read);
  assert(shown.every(setting => setting.group === 'NVIDIA NVENC' && setting.category === 'encoders'));
  // nvenc.rs Tuning::new defaults.
  for (const [key, value] of [
    ['nvenc_preset', '1'], ['nvenc_twopass', 'quarter_res'], ['nvenc_rate_control', 'cbr'], ['nvenc_target_quality', 0], ['nvenc_peak_bitrate_ratio', '1.5'],
    ['nvenc_spatial_aq', true], ['nvenc_temporal_aq', false], ['nvenc_split_encode', 'auto'], ['nvenc_frame_budget_guard', true],
    ['nvenc_realtime_hags', true], ['nvenc_latency_over_power', true],
    ['nvenc_vbv_increase', 0], ['nvenc_weighted_prediction', false], ['nvenc_h264_cavlc', false], ['nvenc_insert_filler_data', false],
    ['nvenc_enable_min_qp', false], ['nvenc_min_qp_h264', 19], ['nvenc_min_qp_hevc', 23], ['nvenc_min_qp_av1', 23],
  ]) {
    assert.equal(settings.get(key).default, value, key);
  }
  assert.deepEqual(settings.get('nvenc_split_encode').control.options.map(option => option.value),
    ['auto', 'enabled', 'disabled', 'two_strips', 'three_strips', 'four_strips']);
  for (const setting of shown) assert.doesNotMatch(`${setting.label} ${setting.description} ${setting.details ?? ''}`, /experimental|FFmpeg/i, setting.key);
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
  assert.equal(describeDefault(setting).text, 'One for each device on Windows 11 23H2 or later, otherwise a monitor');
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

test('the troubleshooting fold counts customized and unsaved settings and opens for unsaved ones', () => {
  const { foldStatus } = values;
  const group = schema.settings.filter(setting => setting.group === 'Capture troubleshooting');
  const none = foldStatus(group, {}, {}, new Set());
  assert.deepEqual(none, { customized: 0, unsaved: 0, mustOpen: false });
  // wgc_direct_encoder_input defaults on, so storing false customizes it.
  const stored = foldStatus(group, {}, { wgc_direct_encoder_input: false }, new Set());
  assert.deepEqual(stored, { customized: 1, unsaved: 0, mustOpen: false });
  const edited = foldStatus(group, { wgc_direct_encoder_input: false }, {}, new Set(['wgc_direct_encoder_input']));
  assert.deepEqual(edited, { customized: 1, unsaved: 1, mustOpen: true });
  // An edit back to the default is unsaved but no longer customized.
  const reverted = foldStatus(group, { wgc_direct_encoder_input: true }, { wgc_direct_encoder_input: false }, new Set(['wgc_direct_encoder_input']));
  assert.deepEqual(reverted, { customized: 0, unsaved: 1, mustOpen: true });
});

test('bitrate and pacing controllers say which one wins', () => {
  // The finer points live behind More; descriptions are capped at two short sentences.
  const says = (key, pattern) => assert.match(settings.get(key).details, pattern, key);
  says('link_rate_adaptation', /Adapt bitrate on the host wins[^.]*sets its own bitrate takes over/);
  says('host_adaptive_bitrate', /replaces Lower the bitrate when the link cannot carry it/);
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
  assert.equal(activate.visibleWhen({}), true, 'unset is off before Windows 11 23H2');
  assert.equal(activate.visibleWhen({ virtual_display_mode: '' }), true);
  assert.equal(activate.visibleWhen({ virtual_display_mode: 'disabled' }), true);
  assert.equal(activate.visibleWhen({ virtual_display_mode: 'per_client' }), false);
  assert.equal(activate.visibleWhen({ virtual_display_mode: 'shared' }), false);
  // A device or app set to Off still gets a virtual display while this is on, so the row stays.
  assert.equal(activate.visibleWhen({ virtual_display_mode: 'per_client', dd_activate_virtual_display: true }), true);
  assert.equal(activate.visibleWhen({ virtual_display_mode: 'shared', dd_activate_virtual_display: 'true' }), true);
  assert.equal(activate.visibleWhen({ virtual_display_mode: 'shared', dd_activate_virtual_display: false }), false);
  assert.ok(activate.inactiveHint);
});

test('library counts and time limits reject fractions instead of saving values the host would ignore', () => {
  // These are Config::integer inputs in core/src/steam.rs and core/src/playnite.rs.
  for (const key of [
    'steam_recent_games', 'steam_recent_max_age_days',
    'playnite_recent_games', 'playnite_recent_max_age_days', 'playnite_autosync_delete_after_days',
    'playnite_focus_attempts', 'playnite_focus_timeout_secs', 'playnite_exit_wait_secs',
  ]) {
    const setting = settings.get(key);
    assert.equal(numberError(setting, '2.5'), 'Enter a whole number.', key);
    assert.equal(numberError(setting, '2'), '', key);
    assert.equal(numberError(setting, '0'), '', key);
    assert.equal(numberError(setting, ''), '', key);
  }
});

test('Epic and GOG scans default on, game folders default empty and their exclusions follow them', () => {
  // The host's defaults, in library::Settings::from_config (core/src/library.rs).
  assert.equal(settings.get('library_epic_enabled').default, true);
  assert.equal(settings.get('library_gog_enabled').default, true);
  const folders = settings.get('library_local_folders');
  assert.equal(folders.control.kind, 'list');
  assert.deepEqual(folders.default, []);
  const exclude = settings.get('library_local_exclude');
  assert.equal(exclude.control.kind, 'list');
  assert.equal(exclude.visibleWhen({}), false);
  assert.equal(exclude.visibleWhen({ library_local_folders: [] }), false);
  assert.equal(exclude.visibleWhen({ library_local_folders: ['D:\\Games'] }), true);
  assert.equal(exclude.visibleWhen({ library_local_folders: '["D:\\\\Games"]' }), true);
  assert.ok(exclude.inactiveHint);
  for (const key of ['library_epic_enabled', 'library_gog_enabled', 'library_local_folders', 'library_local_exclude']) {
    assert.equal(settings.get(key).category, 'library', key);
  }
});

test('every launcher has the same on switch and sync as Steam, all on by default', () => {
  // The host's defaults: core/src/library.rs Settings::from_config and core/src/steam.rs.
  const launchers = ['epic', 'battlenet', 'ea', 'ubisoft', 'gog', 'microsoft'].map(id => `library_${id}_enabled`);
  for (const key of [...launchers, 'library_auto_sync', 'library_autosync_remove_uninstalled', 'steam_enabled', 'steam_auto_sync']) {
    const setting = settings.get(key);
    assert.equal(setting.default, true, key);
    assert.equal(setting.control.kind, 'toggle', key);
    assert.equal(setting.category, 'library', key);
  }
  for (const key of launchers) {
    assert.equal(settings.get(key).group, 'Launchers', key);
    assert.match(settings.get(key).description, /^Adds the games installed with .+ to the library, with their covers/, key);
  }
  const remove = settings.get('library_autosync_remove_uninstalled');
  assert.equal(remove.visibleWhen({}), true);
  assert.equal(remove.visibleWhen({ library_auto_sync: false }), false);
  assert.equal(settings.get('steam_auto_sync').visibleWhen({}), true);
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

test('file transfer is off by default, its options show once it is on, and the draft round-trips', () => {
  const transfer = settings.get('file_transfer');
  assert.equal(transfer.default, false);
  assert.doesNotMatch(transfer.label, /Experimental/);
  assert.equal(transfer.advanced, false);
  const defaults = { file_transfer_dir: '', file_transfer_max_mb: 4096, file_transfer_stream_mbps: 50 };
  for (const [key, value] of Object.entries(defaults)) {
    const setting = settings.get(key);
    assert.equal(setting.default, value, key);
    assert.equal(setting.category, 'network', key);
    assert.equal(setting.visibleWhen({}), false, key);
    assert.equal(setting.visibleWhen({ file_transfer: 'enabled' }), true, key);
    assert.equal(differsFromDefault(setting, {}, {}), false, key);
  }
  assert.equal(settings.get('file_transfer_dir').control.browse, 'directory');
  // 0 is a valid rate: uploads run unpaced.
  assert.equal(numberError(settings.get('file_transfer_stream_mbps'), '0'), '');
  assert.notEqual(numberError(settings.get('file_transfer_max_mb'), '0'), '');
  const saved = { file_transfer: 'disabled' };
  const draft = { file_transfer: true, file_transfer_dir: 'D:\\Inbox', file_transfer_max_mb: 1024, file_transfer_stream_mbps: 0 };
  const sent = computeChanges(settings, draft, saved);
  assert.deepEqual(sent, draft);
  assert.deepEqual(computeChanges(settings, draft, { ...saved, ...sent }), {});
  assert.deepEqual(computeChanges(settings, { file_transfer: null }, sent), { file_transfer: null });
});

test('USB forwarding is live, unlabelled and off, and takes 1 to 16 devices once it is on', () => {
  const forwarding = settings.get('usb_forwarding');
  assert.deepEqual([forwarding.category, forwarding.group, forwarding.default, forwarding.advanced], ['general', 'USB devices', false, false]);
  assert.doesNotMatch(forwarding.label, /Experimental/);
  assert.equal(pending.some(setting => setting.key.startsWith('usb_')), false);
  const devices = settings.get('usb_max_devices');
  assert.deepEqual([devices.category, devices.group, devices.default], ['general', 'USB devices', 4]);
  assert.equal(devices.visibleWhen({}), false);
  assert.equal(devices.visibleWhen({ usb_forwarding: 'enabled' }), true);
  // core/src/config.rs: 1 to 16.
  for (const ok of ['1', '16']) assert.equal(numberError(devices, ok), '', ok);
  for (const bad of ['0', '17']) assert.notEqual(numberError(devices, bad), '', bad);
});

test('sleep after idle takes 0 or 5 to 1440 minutes, as the host does, and its suspended option follows it', () => {
  const minutes = settings.get('sleep_after_idle_minutes');
  assert.deepEqual([minutes.category, minutes.group, minutes.default, minutes.advanced], ['general', 'Power', 0, false]);
  // core/src/config.rs: 0 (off) or between 5 and 1440.
  for (const ok of ['0', '5', '30', '1440', '']) assert.equal(numberError(minutes, ok), '', ok);
  for (const bad of ['3', '4']) assert.equal(numberError(minutes, bad), 'Enter 0 (off) or 5 to 1440 minutes.', bad);
  assert.equal(numberError(minutes, '1441'), 'Enter a number from 0 to 1440.');
  assert.equal(numberError(minutes, '7.5'), 'Enter a whole number.');
  const suspended = settings.get('sleep_while_suspended');
  assert.equal(suspended.default, false);
  assert.equal(suspended.visibleWhen({}), false);
  assert.equal(suspended.visibleWhen({ sleep_after_idle_minutes: '30' }), true);
  assert.equal(suspended.visibleWhen({ sleep_after_idle_minutes: 0 }), false);
});

test('the keyboard follows text fields without their text, typing with the device layout', () => {
  const follows = settings.get('keyboard_follows_focus');
  const surrounding = settings.get('text_input_surrounding_text');
  const layout = settings.get('keyboard_layout');
  assert.deepEqual([follows.default, surrounding.default, layout.default], [true, false, 'client']);
  assert.doesNotMatch(surrounding.label, /Experimental/);
  assert.doesNotMatch(surrounding.description, /Experimental/);
  for (const setting of [follows, surrounding, layout]) {
    assert.equal(setting.group, 'Keyboard', setting.key);
    assert.equal(setting.visibleWhen({ keyboard: false }), false, setting.key);
    assert.equal(setting.visibleWhen({}), true, setting.key);
  }
  assert.match(surrounding.description, /never from password fields/);
  assert.equal(surrounding.visibleWhen({ keyboard_follows_focus: false }), false);
});

test('image clipboard is on by default, unlabelled, and its limit follows it as the host checks it', () => {
  const images = settings.get('clipboard_image');
  assert.equal(images.default, true);
  assert.doesNotMatch(images.label, /Experimental/);
  const limit = settings.get('clipboard_image_max_mb');
  assert.deepEqual([limit.category, limit.group, limit.default], ['general', 'Clipboard', 16]);
  assert.equal(limit.visibleWhen({}), true);
  assert.equal(limit.visibleWhen({ clipboard_image: 'disabled' }), false);
  // core/src/config.rs: 1 to 64 MiB.
  for (const ok of ['1', '16', '64']) assert.equal(numberError(limit, ok), '', ok);
  for (const bad of ['0', '65', '1.5']) assert.notEqual(numberError(limit, bad), '', bad);
});

test('shared folders are a live list beside file transfer, empty by default', () => {
  const folders = settings.get('shared_folders');
  assert.deepEqual([folders.category, folders.group, folders.default], ['network', 'Shared folders', '[]']);
  assert.doesNotMatch(folders.label, /Experimental/);
  // Shared folders don't need File transfer on.
  assert.equal(folders.visibleWhen, undefined);
});

// The 3.0 settings whose features have shipped (settings30.rs keeps the same list).
const live = [
  'audio_surround_714',
  'dd_rotation_option',
  'dd_manual_rotation',
  // Input: pen barrel roll, the device-drawn cursor and touch profiles.
  'pen_barrel_roll',
  'client_cursor',
  'touch_mode',
  'touch_sensitivity',
  'touch_gestures',
  // Nucleus Co-op.
  'nucleus_path',
  'nucleus_coop_players',
];

test('settings still pending stay out of the console, and shipped ones are in it', () => {
  assert.equal(pending.length, 13);
  for (const setting of pending) {
    const shipped = live.includes(setting.key);
    assert.equal(setting.pending === true, !shipped, setting.key);
    // Always-on keys are pending as switches but readable under Debug switches.
    assert.equal(settings.has(setting.key), shipped || setting.key === 'qr_pairing', setting.key);
    assert.ok(schema.categories.some(category => category.id === setting.category), setting.key);
  }
});

test('touch profiles are unlabelled and their options show only as a mouse or trackpad', () => {
  const mode = settings.get('touch_mode');
  assert.doesNotMatch(mode.label, /Experimental/);
  assert.match(mode.description, /Needs native pen and touch/);
  assert.equal(mode.default, 'client');
  for (const key of ['touch_sensitivity', 'touch_gestures']) {
    const setting = settings.get(key);
    assert.equal(setting.visibleWhen({}), false, key);
    assert.equal(setting.visibleWhen({ touch_mode: 'client' }), false, key);
    assert.equal(setting.visibleWhen({ touch_mode: 'mouse' }), true, key);
    assert.equal(setting.visibleWhen({ touch_mode: 'trackpad' }), true, key);
  }
});

test('QR pairing is always on: only a troubleshooting switch, on by default, no Experimental label', () => {
  const setting = pending.find(item => item.key === 'qr_pairing');
  assert.equal(settings.get('qr_pairing').group, 'Debug switches');
  assert.match(settings.get('qr_pairing').description, /only for troubleshooting/);
  assert.equal(setting.default, true);
  assert.doesNotMatch(setting.label + setting.description, /Experimental/);
});

test('7.1.4 surround is unlabelled and says the heights need a 7.1.4 device', () => {
  const setting = settings.get('audio_surround_714');
  assert.doesNotMatch(setting.label, /Experimental/);
  assert.match(setting.description, /Height channels stay silent/);
  assert.equal(setting.default, true);
});

test('display rotation is unlabelled, off by default, and its angle shows only when chosen', () => {
  const option = settings.get('dd_rotation_option');
  const angle = settings.get('dd_manual_rotation');
  for (const setting of [option, angle]) {
    assert.doesNotMatch(setting.label, /Experimental/, setting.key);
    assert.doesNotMatch(setting.description, /Experimental/, setting.key);
    assert.equal(setting.group, 'Streamed display', setting.key);
  }
  assert.equal(option.default, 'disabled');
  assert.deepEqual(option.control.options.map(o => o.value), ['disabled', 'auto', 'manual']);
  assert.deepEqual(angle.control.options.map(o => o.value), ['0', '90', '180', '270']);
  assert.equal(angle.visibleWhen({ dd_rotation_option: 'manual' }), true);
  assert.equal(angle.visibleWhen({ dd_rotation_option: 'auto' }), false);
  assert.equal(angle.visibleWhen({}), false);
});

test('PCM and Dolby Digital audio defaults on, keeps Opus for other devices and follows Stream audio', () => {
  const setting = settings.get('audio_passthrough');
  assert.equal(setting.default, true);
  assert.equal(setting.control.kind, 'toggle');
  assert.equal(setting.category, 'audio');
  assert.match(setting.description, /Other devices keep Opus/);
  assert.match(setting.details, /refused with the reason/);
  assert.equal(setting.visibleWhen({}), true);
  assert.equal(setting.visibleWhen({ stream_audio: false }), false);
  assert.equal(differsFromDefault(setting, {}, {}), false);
  assert.equal(differsFromDefault(setting, {}, { audio_passthrough: false }), true);
});

test('the FEC cap and the bandwidth test are live in the console, not pending', () => {
  for (const [key, category, value] of [['fec_max_percentage', 'video', 50], ['bandwidth_probe', 'network', true]]) {
    const setting = settings.get(key);
    assert.equal(setting?.category, category, key);
    assert.equal(setting.default, value, key);
    assert.equal(setting.pending, undefined, key);
    assert.equal(pending.some(candidate => candidate.key === key), false, key);
  }
});

test('pending keys are unique, and Foundation ports are neither credited in the console nor Experimental', () => {
  const keys = pending.map(setting => setting.key);
  assert.equal(new Set(keys).size, keys.length);
  for (const setting of pending) {
    assert.doesNotMatch(`${setting.label} ${setting.description} ${setting.details ?? ''}`, /Foundation|Annex/, setting.key);
  }
  for (const key of ['hdr_dynamic_metadata', 'fec_max_percentage', 'audio_passthrough']) {
    const setting = settings.get(key) ?? pending.find(candidate => candidate.key === key);
    assert.doesNotMatch(`${setting.label} ${setting.description} ${setting.details ?? ''}`, /Experimental/, key);
  }
});

test('haptics from game audio are off by default, unlabelled, and their options follow the toggle', () => {
  const toggle = settings.get('audio_haptics');
  assert.equal(toggle.default, false);
  assert.doesNotMatch(toggle.label, /Experimental/);
  assert.equal(toggle.group, 'Controllers');
  assert.equal(toggle.visibleWhen({}), true);
  // Generated haptics go out with game vibration, so they hide with it.
  assert.equal(toggle.visibleWhen({ forward_rumble: false }), false);
  assert.equal(toggle.visibleWhen({ controller: 'disabled' }), false);
  const defaults = { audio_haptics_strength: 100, audio_haptics_curve: 'linear', audio_haptics_gate: -50 };
  for (const [key, value] of Object.entries(defaults)) {
    const setting = settings.get(key);
    assert.equal(setting.default, value, key);
    assert.equal(setting.category, 'input', key);
    assert.equal(setting.advanced, true, key);
    assert.equal(setting.visibleWhen({}), false, key);
    assert.equal(setting.visibleWhen({ audio_haptics: 'on' }), true, key);
    assert.equal(setting.visibleWhen({ audio_haptics: 'on', forward_rumble: 'off' }), false, key);
    assert.equal(differsFromDefault(setting, {}, {}), false, key);
  }
  // The ranges the host checks on save (audio_haptics::check).
  const strength = settings.get('audio_haptics_strength');
  assert.equal(numberError(strength, '0'), '');
  assert.equal(numberError(strength, '200'), '');
  assert.notEqual(numberError(strength, '201'), '');
  const gate = settings.get('audio_haptics_gate');
  assert.equal(numberError(gate, '-80'), '');
  assert.notEqual(numberError(gate, '-9'), '');
  assert.deepEqual(settings.get('audio_haptics_curve').control.options.map(option => option.value), ['linear', 'soft', 'punchy']);
  const draft = { audio_haptics: true, audio_haptics_strength: 150, audio_haptics_curve: 'punchy', audio_haptics_gate: -40 };
  const sent = computeChanges(settings, draft, {});
  assert.deepEqual(sent, draft);
  assert.deepEqual(computeChanges(settings, draft, sent), {});
});

test('Steam and Playnite add every installed game unless set to false, and fold their recent-games mode', () => {
  for (const prefix of ['steam', 'playnite']) {
    const all = settings.get(`${prefix}_sync_all_installed`);
    assert.equal(all.default, true, prefix);
    assert.equal(all.advanced, true, prefix);
    const recent = settings.get(`${prefix}_recent_games`);
    assert.equal(recent.advanced, true, prefix);
    assert.equal(recent.visibleWhen({}), false, `${prefix}: an absent key means every installed game`);
    assert.equal(recent.visibleWhen({ [`${prefix}_sync_all_installed`]: false }), true, prefix);
  }
  assert.equal(settings.get('steam_exclude_games').advanced, true);
  assert.equal(settings.get('steam_enabled').group, 'Steam options');
});

test('the host virtual display scale says a device that asks for its own scale gets it', () => {
  // rlDisplayScale (Nightjar's PC display scaling) wins over dd_virtual_display_scale.
  assert.match(settings.get('dd_virtual_display_scale').details, /A device that asks for its own scale \(PC display scaling in Nightjar\) gets that one\./);
});

// Issue 56: a settings file with Usage and Quality preset on "Driver default" ran AMD's transcoding profile
// (AV1 latency mode off), and Maximum frame size 0 let keyframes outgrow error correction, with nothing
// in the console saying so.
const { withPreset } = await server.ssrLoadModule('/src/lib/stream-presets.ts');
const { caution, currentValues, handSetCautions } = values;
const amfValues = (described, config, native = ['amf']) => ({
  ...currentValues(described, {}, config),
  [types.NATIVE_ENCODERS]: native,
});
const costs = (config, native) =>
  handSetCautions(schema.settings, amfValues(schema.settings, config, native), {}, config).map(({ setting }) => setting.key);

test('AMF values that cost the stream say so; the defaults and costless choices say nothing', () => {
  const reported = {
    amd_usage: 'auto', amd_quality: 'auto', amd_rc: 'auto', amd_max_frame_size: 0,
    amd_enforce_hrd: 'enabled', amd_vbaq: 'auto', amd_av1_latency_mode: 'auto',
  };
  assert.deepEqual(costs(reported), ['amd_usage', 'amd_max_frame_size']);
  assert.match(caution(settings.get('amd_usage'), amfValues(schema.settings, reported)), /transcoding profile/);
  assert.match(caution(settings.get('amd_max_frame_size'), amfValues(schema.settings, reported)), /error correction/);
  assert.deepEqual(costs({}), []);
  // AMF settings do not apply where Automatic uses NVENC.
  assert.deepEqual(costs(reported, ['nvenc']), []);
  for (const [config, key, text] of [
    [{ amd_usage: 'transcoding' }, 'amd_usage', /queues frames/],
    [{ amd_usage: 'lowlatency_high_quality' }, 'amd_usage', /0\.9 ms per AV1 frame/],
    [{ amd_quality: 'quality' }, 'amd_quality', /12 ms on AV1/],
    [{ amd_rc: 'qvbr' }, 'amd_rc', /pre-analysis/],
    [{ amd_preanalysis: true }, 'amd_preanalysis', /20 ms per 4K frame/],
    [{ amd_lowlatency_mode: 'false' }, 'amd_lowlatency_mode', /0\.4 ms/],
    [{ amd_av1_latency_mode: 'none' }, 'amd_av1_latency_mode', /1\.3 ms per AV1 frame/],
  ]) {
    assert.deepEqual(costs(config), [key], JSON.stringify(config));
    assert.match(caution(settings.get(key), amfValues(schema.settings, config)), text, key);
  }
  for (const config of [
    { amd_usage: 'ultralowlatency' }, { amd_usage: 'not-a-usage' }, { amd_usage: 'webcam' }, { amd_quality: 'balanced' },
    { amd_rc: 'cbr' }, { amd_max_frame_size: 2 }, { amd_lowlatency_mode: 'enabled' },
    { amd_av1_latency_mode: 'lowest' }, { amd_enforce_hrd: 'enabled' },
  ]) assert.deepEqual(costs(config), [], JSON.stringify(config));
});

test('a preset that removes the frame cap shows it on the row, not as a hand-set cost', () => {
  const described = withPreset(schema.settings, 'reference');
  const config = { stream_preset: 'reference' };
  const current = amfValues(described, config);
  const cap = described.find(setting => setting.key === 'amd_max_frame_size');
  assert.match(caution(cap, current), /error correction/);
  assert.deepEqual(handSetCautions(described, current, {}, config), []);
});

test('a dismissed caution stays dismissed when another listed one is fixed, and a new costly value shows again', () => {
  const { cautionPairs, cautionsPending, dismissCautions } = values;
  const config = { amd_usage: 'auto', amd_max_frame_size: 0 };
  const listed = handSetCautions(schema.settings, amfValues(schema.settings, config), {}, config);
  const pairs = cautionPairs(listed, amfValues(schema.settings, config));
  assert.deepEqual(pairs, ['amd_usage=auto', 'amd_max_frame_size=0']);
  assert.equal(cautionsPending(pairs, []), true);
  const dismissed = dismissCautions(pairs, []);
  assert.equal(cautionsPending(pairs, dismissed), false);
  // Usage fixed: the frame cap the user already dismissed stays dismissed.
  assert.equal(cautionsPending(['amd_max_frame_size=0'], dismissed), false);
  // Another costly value, or a listed setting moved to another costly value, shows the banner again.
  assert.equal(cautionsPending([...pairs, 'amd_preanalysis=true'], dismissed), true);
  assert.equal(cautionsPending(['amd_usage=transcoding', 'amd_max_frame_size=0'], dismissed), true);
  assert.equal(cautionsPending([], dismissed), false);
  assert.deepEqual(dismissCautions(['amd_usage=transcoding'], dismissed), [...pairs, 'amd_usage=transcoding']);
});

test('PyroWave encode overlap is a Codecs switch, off by default, shown with PyroWave', () => {
  const overlap = settings.get('pyrowave_encode_overlap');
  assert.deepEqual([overlap.category, overlap.group, overlap.control.kind, overlap.default], ['video', 'Codecs', 'toggle', false]);
  assert.equal(overlap.visibleWhen({}), true);
  assert.equal(overlap.visibleWhen({ pyrowave: 'true' }), true);
  assert.equal(overlap.visibleWhen({ pyrowave: 'false' }), false);
  assert.match(overlap.details, /one frame at a time/);
});
