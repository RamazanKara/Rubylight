import assert from 'node:assert/strict';
import { test } from 'node:test';
import { createTestServer } from './test-server.mjs';

globalThis.location = new URL('http://localhost/');
globalThis.window = new EventTarget();
const server = await createTestServer();
const { default: HostReadiness } = await server.ssrLoadModule('/src/components/overview/HostReadiness.svelte');
const { render } = await server.ssrLoadModule('svelte/server');
const metadata = {
  encoder_status: { state: 'failed', h264: false, hevc: false, av1: false, pyrowave: false },
  warnings: [{ code: 'video_encoder', message: 'No video encoder available: AMF error 1. Retrying.' }],
  virtual_display: { capable: true, ready: true, reason: '', protocol: 'idd-push' },
  audio_enabled: false,
};
const html = (changes = {}) => render(HostReadiness, {
  props: { metadata: { ...metadata, ...changes }, error: null, checking: false, onrecheck: () => {} },
}).body;

test('host readiness shows the probe error even while a retry is running', () => {
  for (const state of ['failed', 'checking']) {
    const result = html({ encoder_status: { ...metadata.encoder_status, state } });
    assert.match(result, /No video encoder available: AMF error 1\. Retrying\./);
    assert.match(result, /Check the selected encoder and graphics adapter/);
    assert.match(result, /href="\/settings\/encoders"/);
    assert.doesNotMatch(result, /restart the host|Testing which codecs/);
  }
});

test('host readiness clears the warning when encoders recover', () => {
  const result = html({ warnings: [], encoder_status: { ...metadata.encoder_status, state: 'ready', h264: true } });
  assert.doesNotMatch(result, /No video encoder available|Retrying/);
  assert.match(result, /Encodes H\.264\./);
});

test('host readiness escapes driver errors', () => {
  const result = html({ warnings: [{ code: 'video_encoder', message: 'No video encoder available: <script>alert(1)</script>. Retrying.' }] });
  assert.match(result, /&lt;script/);
  assert.doesNotMatch(result, /<script>alert/);
});

test('host readiness writes notes in the text face and opens them only for problems', () => {
  const reason = 'Only the Rubylight service may open the virtual display driver.';
  const healthy = { warnings: [], encoder_status: { ...metadata.encoder_status, state: 'ready', h264: true } };
  const ready = html({ ...healthy, virtual_display: { capable: true, ready: true, reason } });
  assert.match(ready, new RegExp(`<p class="detail[^"]*">${reason}</p>`));
  assert.doesNotMatch(ready, /class="[^"]*mono/);
  assert.doesNotMatch(ready, /<details open/);
  const broken = html({ ...healthy, virtual_display: { capable: false, ready: false, reason } });
  assert.match(broken, /<details open[^>]*>\s*<summary[^>]*>Details<\/summary>\s*<p class="detail[^"]*">Only the Rubylight/);
  assert.match(broken, /Run Rubylight setup again to repair the installation/);
});

test('host readiness says an updated display driver needs a Windows restart', () => {
  const reason = "Rubylight's display driver was updated and runs only after Windows restarts. Restart Windows, then start the stream again. If this persists after a restart, run Rubylight setup again to repair the display driver.";
  const healthy = { warnings: [], encoder_status: { ...metadata.encoder_status, state: 'ready', h264: true } };
  const pending = html({ ...healthy, virtual_display: { capable: true, ready: false, reason, protocol: 'idd-push' } });
  assert.match(pending, /Restart needed/);
  assert.match(pending, /Restart Windows, then start the stream again\./);
  assert.match(pending, /If this persists after a restart, run Rubylight setup again to repair the display driver\./);
  assert.match(pending, /<details open/);
  assert.doesNotMatch(pending, /driver is missing or isn't running/);
});

test('host readiness no longer shows a capture method or a physical display', () => {
  const result = html({ warnings: [], encoder_status: { ...metadata.encoder_status, state: 'ready', h264: true } });
  assert.match(result, /Streams use a virtual display\./);
  assert.doesNotMatch(result, /Screen capture|Windows Graphics Capture|Desktop Duplication|physical display/);
});
