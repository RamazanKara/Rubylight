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
const { describeDefault, numberError } = await server.ssrLoadModule('/src/components/settings/values.ts');
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
