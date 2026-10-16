import assert from 'node:assert/strict';
import { after, test } from 'node:test';
import { createServer } from 'vite';

globalThis.location = new URL('http://localhost/');
globalThis.window = new EventTarget();
const server = await createServer({
  server: { middlewareMode: true, hmr: false }, appType: 'custom',
  optimizeDeps: { noDiscovery: true, include: [] },
});
after(() => server.close());
const { schema } = await server.ssrLoadModule('/src/lib/settings-schema.ts');
const { default: SettingControl } = await server.ssrLoadModule('/src/components/settings/SettingControl.svelte');
const { render } = await server.ssrLoadModule('svelte/server');
const { describeDefault, normalise, numberError } = await server.ssrLoadModule('/src/components/settings/values.ts');
const { session } = await server.ssrLoadModule('/src/lib/session.svelte.ts');

const intel = 'Intel(R) HD Graphics 530';
const radeon = 'AMD Radeon R7 M370';
const gpu = (name) => ({ name, vendor: name === intel ? 0x8086 : 0x1002, dedicated_memory: 0, luid: [1, 0], pnp_id: null });
const metadata = {
  gpus: [gpu(intel), gpu(radeon)], gpu_error: null,
  encoder_status: { state: 'failed', h264: false, hevc: false, av1: false, pyrowave: false },
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

test('GPU enumeration failures are shown', () => {
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
  ['packetsize', [0, 256, 1400], [-1, 1, 255, 1401]],
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
