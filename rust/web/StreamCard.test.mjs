import assert from 'node:assert/strict';
import { after, test } from 'node:test';
import { createServer } from 'vite';

globalThis.location = new URL('http://localhost/');
globalThis.window = new EventTarget();
const server = await createServer({
  server: { middlewareMode: true }, appType: 'custom',
  optimizeDeps: { noDiscovery: true, include: [] },
});
after(() => server.close());
const { default: StreamCard } = await server.ssrLoadModule('/src/components/overview/StreamCard.svelte');
const { render } = await server.ssrLoadModule('svelte/server');
const stream = {
  device_name: 'Test client', state: 'RUNNING', width: 3840, height: 2160, fps: 116,
  video_format: 1, encoder: 'amf', encoder_bitrate_kbps: 100_000,
  pyrowave_minimum_kbps: null, pyrowave_recommended_kbps: null,
  uptime_seconds: 1, performance: { sample_frames: 0, history: [] }, warnings: [],
};
const html = (changes = {}) => render(StreamCard, {
  props: { stream: { ...stream, ...changes }, ondisconnect: () => {} },
}).body;

test('stream card renders the backend and actionable warnings as escaped text', () => {
  const result = html({ warnings: [
    { code: 'capture', message: 'Capturing with Desktop Duplication: WGC unavailable. Sign in again.' },
    { code: 'encoder', message: 'Driver failed: <script>alert(1)</script>' },
  ] });
  assert.match(result, /Encoder: AMD AMF/);
  assert.match(result, /role="status"[^>]*>Capturing with Desktop Duplication/);
  assert.match(result, /&lt;script/);
  assert.doesNotMatch(result, /<script>alert/);
});

test('recovered warnings disappear without invented timing measurements', () => {
  const result = html();
  assert.doesNotMatch(result, /role="status"/);
  assert.match(result, /No frames sent in the last 2 seconds/);
});

test('PyroWave quality warning uses the current encoder bitrate', () => {
  const result = html({ video_format: 3, pyrowave_minimum_kbps: 200_000, pyrowave_recommended_kbps: 300_000 });
  assert.match(result, /PyroWave bitrate is too low/);
  assert.match(result, /severe detail loss/);
});

test('the disconnect button names the device whose stream it ends', () => {
  assert.match(html(), /Disconnect(<!---->)?<span class="visually-hidden[^"]*">\s*Test client<\/span>/);
});

const spread = (mean_ms, p99_ms) => ({ mean_ms, p95_ms: p99_ms, p99_ms });
const measured = (frames) => ({
  sample_frames: 240, history: [], fps: 120, encode_p95_ms: 2.5,
  host_processing_mean_ms: 3.1, host_processing_p99_ms: 4.2, frame_age_mean_ms: 1.2,
  stages: { frames, window_seconds: 5, present_to_sent: spread(9.84, 12.31), moonlight_host: spread(3.1, 4.2) },
});

test('present to sent shows new pictures from Windows present to the last packet beside Moonlight host processing', () => {
  const result = html({ performance: measured(600) });
  assert.match(result, /<dt[^>]*>Present to sent<\/dt>/);
  assert.match(result, /Present to sent<\/dt>\s*<dd class="value num[^"]*">9\.8 ms</);
  assert.match(result, /Windows' present to the last packet sent, for new pictures; 99% below\s*<span class="num[^"]*">12 ms<\/span>/);
  assert.doesNotMatch(result, /punktfunk/i);
});

test('present to sent is not invented for a still picture or a stream without frames', () => {
  const still = html({ performance: measured(0) });
  assert.match(still, /Present to sent<\/dt>\s*<dd class="value num[^"]*">–<\/dd>/);
  assert.match(still, /No new pictures in the last <span class="num[^"]*">5<\/span> seconds/);
  const idle = html();
  assert.match(idle, /Present to sent<\/dt>\s*<dd class="value num[^"]*">–<\/dd>/);
  assert.doesNotMatch(idle, /No new pictures/);
});
