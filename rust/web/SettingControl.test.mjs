import assert from 'node:assert/strict';
import { test } from 'node:test';
import { createTestServer } from './test-server.mjs';

globalThis.location = new URL('http://localhost/');
globalThis.window = new EventTarget();
const server = await createTestServer();
const { schema } = await server.ssrLoadModule('/src/lib/settings-schema.ts');
const { default: SettingControl } = await server.ssrLoadModule('/src/components/settings/SettingControl.svelte');
const { render } = await server.ssrLoadModule('svelte/server');
const { computeChanges, describeDefault, normalise, numberError, textError, timeOfDay } = await server.ssrLoadModule('/src/components/settings/values.ts');
const { session } = await server.ssrLoadModule('/src/lib/session.svelte.ts');

const intel = 'Intel(R) HD Graphics 530';
const radeon = 'AMD Radeon R7 M370';
const gpu = (name) => ({ name, vendor: name === intel ? 0x8086 : 0x1002, dedicated_memory: 0, luid: [1, 0], pnp_id: null });
const metadata = {
  gpus: [gpu(intel), gpu(radeon)], gpu_error: null,
  encoder_status: { state: 'failed', h264: false, hevc: false, av1: false, pyrowave: false },
  capture_status: { displays: [{ display_name: 'DISPLAY1', adapter: intel }], error: null },
};
function graphicsControl(changes = {}, value = '') {
  session.metadata = { ...metadata, ...changes };
  try {
    const setting = schema.settings.find((setting) => setting.key === 'adapter_name');
    return render(SettingControl, { props: { setting, id: setting.key, value, onchange: () => {} } }).body;
  } finally {
    session.metadata = null;
  }
}

test('graphics card choices include a hybrid render-only GPU even when encoder probing fails', () => {
  const html = graphicsControl();
  assert.match(html, /value="Intel\(R\) HD Graphics 530"/);
  assert.match(html, /value="AMD Radeon R7 M370"/);
  assert.match(html, />Automatic<\/option>/);
});

test('graphics card inventory remains available when display enumeration fails', () => {
  const html = graphicsControl({ capture_status: { displays: null, error: 'Display query failed' } });
  assert.match(html, /value="AMD Radeon R7 M370"/);
  assert.doesNotMatch(html, /Could not list displays/);
});

test('GPU enumeration failures are shown independently of connected displays', () => {
  const html = graphicsControl({ gpus: null, gpu_error: 'GPU query failed' });
  assert.match(html, /Could not list graphics cards: GPU query failed/);
  assert.doesNotMatch(html, /value="Intel\(R\) HD Graphics 530"/);
});

test('duplicate adapter names keep the saved spelling selected once', () => {
  const saved = 'intel(r) hd graphics 530';
  const html = graphicsControl({ gpus: [gpu(intel), gpu(intel)] }, saved);
  assert.equal(html.split(`value="${saved}"`).length - 1, 1);
  assert.doesNotMatch(html, /Saved custom choice/);
});

test('a temporarily unavailable saved GPU is preserved', () => {
  const html = graphicsControl({ gpus: [] }, radeon);
  assert.match(html, /value="AMD Radeon R7 M370"/);
  assert.match(html, /Saved custom choice/);
});

test('values the fallback console saves select the option the host reads them as', () => {
  for (const [key, saved, option] of [
    ['frame_limiter_auto_virtual_framegen', 'fixed-1000hz', 'vrr'],
    ['rtss_frame_limit_type', 'front-edge-sync', 'front edge sync'],
    ['amd_vbaq', 'true', 'enabled'],
    ['min_log_level', 3, 'warning'],
  ]) {
    const setting = schema.settings.find((setting) => setting.key === key);
    const value = normalise(setting, saved);
    const html = render(SettingControl, { props: { setting, id: key, value, onchange: () => {} } }).body;
    assert.doesNotMatch(html, /Saved custom choice/, key);
    assert.match(html, new RegExp(`<option value="${option}"[^>]*selected`), key);
  }
  // A value the host does not read is still shown as saved.
  const setting = schema.settings.find((setting) => setting.key === 'rtss_frame_limit_type');
  const html = render(SettingControl, { props: { setting, id: setting.key, value: normalise(setting, 'sync'), onchange: () => {} } }).body;
  assert.match(html, /Saved custom choice: sync/);
});

for (const [key, valid, invalid] of [
  ['update_check_interval', [0, 3600, 86400], [-1, 1, 3599]],
]) {
  test(`${key} accepts zero and its supported range`, () => {
    const setting = schema.settings.find((setting) => setting.key === key);
    for (const value of [...valid, ...invalid]) {
      const html = render(SettingControl, { props: { setting, id: key, value, onchange: () => {} } }).body;
      assert.equal(html.includes('aria-invalid="true"'), invalid.includes(value), String(value));
    }
  });
}

test('the update check time is a time field beside the interval that takes only a time of day', () => {
  const settings = schema.settings.filter((setting) => setting.group === 'Updates').map((setting) => setting.key);
  assert.deepEqual(settings.slice(0, 2), ['update_check_interval', 'update_check_time']);
  const setting = schema.settings.find((setting) => setting.key === 'update_check_time');
  assert.equal(setting.category, 'general');
  assert.equal(setting.advanced, schema.settings.find((item) => item.key === 'update_check_interval').advanced);
  assert.equal(setting.default, '');
  assert.equal(describeDefault(setting).text, 'Not set');
  // As the host's parse_time_of_day: HH:MM or H:MM on the 24-hour clock.
  for (const [text, time] of [['03:00', '03:00'], ['3:00', '03:00'], ['00:00', '00:00'], ['23:59', '23:59'], [' 05:30 ', '05:30'], ['"05:30"', '05:30']]) {
    assert.equal(timeOfDay(text), time, text);
    assert.equal(textError(setting, text), '', text);
  }
  for (const text of ['24:00', '12:60', '3am', '3', '03:0', '03:000', '003:00', '03:00:00', '-3:00', '3.5', ':30', '10800']) {
    assert.equal(timeOfDay(text), '', text);
    assert.match(textError(setting, text), /is not a time of day\. Enter one as HH:MM/, text);
  }
  // Empty is valid: it clears the time, and the interval decides again.
  assert.equal(textError(setting, ''), '');
  const map = new Map([[setting.key, setting]]);
  assert.deepEqual(computeChanges(map, { update_check_time: '' }, { update_check_time: '03:00' }), { update_check_time: null });
  assert.deepEqual(computeChanges(map, { update_check_time: '05:00' }, {}), { update_check_time: '05:00' });

  const field = (value, invalidText) =>
    render(SettingControl, { props: { setting, id: setting.key, value: normalise(setting, value), invalidText, onchange: () => {} } }).body;
  assert.match(field('03:00'), /<input[^>]*type="time"[^>]*value="03:00"/);
  assert.doesNotMatch(field('03:00'), /aria-invalid/);
  // A saved 3:00 is 03:00 to the host; a time field would show it as empty.
  assert.match(field('3:00'), /value="03:00"/);
  assert.doesNotMatch(field(undefined), /aria-invalid|class="error"/);
  // A value written by hand that the host ignores is named, not hidden.
  const unreadable = field('3am');
  assert.match(unreadable, /aria-invalid="true"/);
  assert.match(unreadable, /“3am” is not a time of day\./);
  // Other text settings stay text fields.
  const name = schema.settings.find((item) => item.key === 'sunshine_name');
  assert.match(render(SettingControl, { props: { setting: name, id: name.key, value: 'PC', onchange: () => {} } }).body, /type="text"/);
});

test('hardware metrics are opt-in with a bounded interval and an optional selection', () => {
  const settings = Object.fromEntries(schema.settings.map((setting) => [setting.key, setting]));
  assert.equal(settings.hardware_metrics.default, false);
  const interval = settings.hardware_metrics_interval;
  assert.equal(interval.default, 1000);
  assert.equal(interval.visibleWhen({}), false);
  assert.equal(interval.visibleWhen({ hardware_metrics: true }), true);
  for (const value of [499, 500, 1000, 10000, 10001]) {
    const html = render(SettingControl, { props: { setting: interval, id: interval.key, value, onchange: () => {} } }).body;
    assert.equal(html.includes('aria-invalid="true"'), value < 500 || value > 10000, String(value));
  }
  const selection = settings.hardware_metrics_selection;
  assert.deepEqual(selection.default, []);
  assert.equal(describeDefault(selection).text, 'All supported readings');
  assert.equal(selection.control.kind, 'list');
  assert.equal(selection.visibleWhen({ hardware_metrics: false }), false);
  assert.equal(selection.visibleWhen({ hardware_metrics: 'enabled' }), true);
});

test('integer controls with larger increments accept any whole number and reject fractions', () => {
  for (const [key, whole, fractional] of [
    ['capture_poll_interval_us', 501, 500.5],
    ['dd_config_revert_delay', 150, 150.5],
  ]) {
    const setting = schema.settings.find(setting => setting.key === key);
    assert(setting, key);
    assert.equal(numberError(setting, String(whole)), '', key);
    assert.equal(numberError(setting, String(fractional)), 'Enter a whole number.', key);
    const html = render(SettingControl, { props: { setting, id: key, value: fractional, onchange: () => {} } }).body;
    assert.match(html, /aria-invalid="true"/);
  }
});

test('a settings row shows its caution under the control', async () => {
  const { default: SettingRow } = await server.ssrLoadModule('/src/components/settings/SettingRow.svelte');
  const setting = schema.settings.find((item) => item.key === 'amd_usage');
  const props = { setting, value: 'auto', stored: true, changed: false, resetting: false, differs: true,
    onchange() {}, onreset() {}, onundo() {}, oninvalid() {} };
  const shown = render(SettingRow, { props: { ...props, caution: 'Adds delay: test.' } }).body;
  assert.match(shown, /data-caution[^>]*>Adds delay: test\.<\/p>/);
  assert.doesNotMatch(render(SettingRow, { props }).body, /data-caution/);
});
