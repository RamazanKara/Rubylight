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
  virtual_display: { capable: false, ready: false, reason: '' },
  capture_status: { virtual_display_configured: false, displays: [], error: null },
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

test('host readiness names driver push and explains its display scope without claiming it is active', () => {
  const capture_status = { ...metadata.capture_status, configured_backend: 'idd-push' };
  const result = html({ capture_status });
  assert.match(result, /Prefers Direct driver push on Rubylight virtual displays/);
  assert.match(result, /Windows capture as fallback/);
  assert.doesNotMatch(result, /Uses the configured capture method/);
});

test('host readiness writes notes in the text face and opens them only for problems', () => {
  const reason = 'Only the Rubylight service may open the virtual display driver.';
  const healthy = { warnings: [], encoder_status: { ...metadata.encoder_status, state: 'ready', h264: true } };
  const off = html({ ...healthy, virtual_display: { capable: false, ready: false, reason } });
  assert.match(off, new RegExp(`<p class="detail[^"]*">${reason}</p>`));
  assert.doesNotMatch(off, /class="[^"]*\bmono\b/);
  assert.doesNotMatch(off, /<details open/);
  const broken = html({
    ...healthy,
    virtual_display: { capable: false, ready: false, reason },
    capture_status: { ...metadata.capture_status, virtual_display_configured: true },
  });
  assert.match(broken, /<details open[^>]*>\s*<summary[^>]*>Details<\/summary>\s*<p class="detail[^"]*">Only the Rubylight/);
  assert.match(broken, /Check that the Rubylight service is running/);
});
