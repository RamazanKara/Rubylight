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
