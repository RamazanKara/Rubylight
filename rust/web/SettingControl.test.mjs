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
const { describeDefault } = await server.ssrLoadModule('/src/components/settings/values.ts');

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
