import assert from 'node:assert/strict';
import { test } from 'node:test';
import { createTestServer } from './test-server.mjs';

globalThis.location = new URL('http://localhost/');
globalThis.window = new EventTarget();
const server = await createTestServer();
const load = async (path) => (await server.ssrLoadModule(path)).default;
const { render } = await server.ssrLoadModule('svelte/server');
const { createRawSnippet } = await server.ssrLoadModule('svelte');
const { rovingIndex } = await server.ssrLoadModule('/src/lib/design/keys.ts');
const html = (text) => createRawSnippet(() => ({ render: () => `<span>${text}</span>` }));
const tabPanel = createRawSnippet((id) => ({ render: () => `<p>Panel ${id()}</p>` }));
const body = async (name, props) => render(await load(`/src/components/${name}.svelte`), { props }).body;

test('arrow keys wrap, Home and End jump, and disabled tabs are skipped', () => {
  assert.equal(rovingIndex(0, 'ArrowRight', 3), 1);
  assert.equal(rovingIndex(2, 'ArrowRight', 3), 0);
  assert.equal(rovingIndex(0, 'ArrowLeft', 3), 2);
  assert.equal(rovingIndex(1, 'Home', 3), 0);
  assert.equal(rovingIndex(0, 'End', 3), 2);
  assert.equal(rovingIndex(0, 'ArrowRight', 3, 'horizontal', (i) => i === 1), 2);
  assert.equal(rovingIndex(1, 'End', 3, 'horizontal', (i) => i === 2), 1);
  assert.equal(rovingIndex(0, 'ArrowDown', 3), null, 'horizontal tabs leave up and down to the page');
  assert.equal(rovingIndex(0, 'ArrowDown', 3, 'vertical'), 1);
  assert.equal(rovingIndex(0, 'Enter', 3), null);
  assert.equal(rovingIndex(0, 'ArrowRight', 2, 'horizontal', () => true), null);
});

test('tabs have one tab stop, and each tab names the panel it controls', async () => {
  const out = await body('Tabs', {
    label: 'Stream settings', selected: 'audio', panel: tabPanel,
    tabs: [{ id: 'video', label: 'Video' }, { id: 'audio', label: 'Audio', badge: 2 }],
  });
  assert.match(out, /role="tablist"[^>]*aria-label="Stream settings"/);
  assert.equal(out.match(/tabindex="0"/g).length, 2, 'the selected tab and its panel');
  assert.match(out, /aria-selected="true"[^>]*tabindex="0"[^>]*>\s*Audio/);
  const controls = out.match(/aria-controls="([^"]+audio)"/)[1];
  assert.match(out, new RegExp(`role="tabpanel" id="${controls}"`));
  assert.match(out, /<p>Panel audio<\/p>/);
});

test('a status is always spelled out, never colour alone', async () => {
  assert.match(await body('StatusPill', { status: 'streaming' }), /Streaming/);
  assert.match(await body('StatusPill', { status: 'error', label: 'Encoder failed' }), /Encoder failed/);
});

test('banners announce themselves only when asked, and a failure as an alert', async () => {
  assert.doesNotMatch(await body('Banner', { title: 'Pairing is on' }), /role=/);
  assert.match(await body('Banner', { tone: 'danger', live: true, title: 'Capture failed' }), /role="alert"/);
  assert.match(await body('Banner', { tone: 'warn', live: true, title: 'No audio' }), /role="status"/);
  assert.match(await body('Banner', { title: 'x', ondismiss: () => {} }), /aria-label="Dismiss"/);
});

test('a dialog is named by its title and described by its description', async () => {
  const out = await body('Dialog', { open: true, title: 'Unpair Pixel 9?', description: 'It needs a new PIN.' });
  const label = out.match(/aria-labelledby="([^"]+)"/)[1];
  const description = out.match(/aria-describedby="([^"]+)"/)[1];
  assert.match(out, new RegExp(`id="${label}"[^>]*>Unpair Pixel 9\\?`));
  assert.match(out, new RegExp(`id="${description}"[^>]*>It needs a new PIN.`));
});

test('a wide table scrolls in a named region that the keyboard can reach', async () => {
  const out = await body('Table', { caption: 'Connected devices', children: html('rows') });
  assert.match(out, /role="region" aria-label="Connected devices" tabindex="0"/);
  assert.match(out, /<caption class="[^"]*visually-hidden[^"]*">Connected devices<\/caption>/);
});

test('one skeleton per region speaks, the rest are hidden', async () => {
  assert.match(await body('Skeleton', { label: 'Loading devices' }), /role="status"[\s\S]*Loading devices/);
  assert.match(await body('Skeleton', { lines: 3 }), /aria-hidden="true"/);
});

test('inputs and selects report an invalid value to assistive tech', async () => {
  assert.match(await body('Input', { id: 'port', invalid: true, describedby: 'port-error' }),
    /aria-invalid="true"[^>]*aria-describedby="port-error"|aria-describedby="port-error"[^>]*aria-invalid="true"/);
  const select = await body('Select', { value: 'b', options: [{ value: 'a', label: 'A' }, { value: 'b', label: 'B' }] });
  assert.match(select, /<option value="b"[^>]*selected/);
  assert.doesNotMatch(select, /aria-invalid/);
});

test('a pairing request shows its address first and apart from the name it chose', async () => {
  const request = { uniqueid: 'a', name: 'Living room TV (from 192.168.4.20)', address: '127.0.0.1', age_seconds: 3, webrtc: true };
  for (const out of [await body('PairingBanner', { requests: [request] }), await body('devices/PendingRequest', { request })]) {
    const from = out.indexOf('Pairing request from this PC (a tunnel or proxy)');
    assert.ok(from >= 0, out);
    assert.ok(from < out.indexOf('Living room TV'), out);
    assert.match(out, /<span class="from[^"]*">Pairing request from this PC/);
    assert.doesNotMatch(out, /127\.0\.0\.1/);
  }
  const banner = await body('PairingBanner', { requests: [{ ...request, address: '192.168.4.30' }] });
  assert.match(banner, /<span class="from[^"]*">Pairing request from 192\.168\.4\.30<\/span>/);
  assert.match(banner, /<strong class="name[^"]*">Living room TV \(from 192\.168\.4\.20\) wants to pair\.<\/strong>/);
  assert.ok(banner.indexOf('Pairing request from') < banner.indexOf('Living room TV'), banner);
});

test('a pairing request from the internet asks the owner to expect it', async () => {
  const request = { uniqueid: 'a', name: 'Phone', address: '203.0.113.7', internet: true, age_seconds: 3 };
  const banner = await body('PairingBanner', { requests: [request] });
  assert.match(banner, /Phone wants to pair from the internet\./);
  assert.match(banner, /only if you expect this device/);
  assert.match(await body('devices/PendingRequest', { request }), /From the internet: enter its PIN only if you expect this device\./);
  const tunnelled = await body('devices/PendingRequest', { request: { ...request, address: '127.77.3.4' } });
  assert.match(tunnelled, /Pairing request from a device over the internet/);
  const local = await body('PairingBanner', { requests: [{ ...request, address: '192.168.4.30', internet: false }] });
  assert.doesNotMatch(local, /internet|expect/);
});

test('a linked card is one named link', async () => {
  const out = await body('Card', { href: '/devices/pixel', label: 'Pixel 9 settings', children: html('Pixel 9') });
  assert.match(out, /<a [^>]*href="\/devices\/pixel"[^>]*aria-label="Pixel 9 settings"|<a [^>]*aria-label="Pixel 9 settings"[^>]*href="\/devices\/pixel"/);
});
