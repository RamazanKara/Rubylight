import assert from 'node:assert/strict';
import { readFile } from 'node:fs/promises';
import { test } from 'node:test';
import { createTestServer } from './test-server.mjs';

globalThis.location = new URL('http://localhost/');
globalThis.window = new EventTarget();
const server = await createTestServer();
const { render } = await server.ssrLoadModule('svelte/server');
const { default: HostReadiness } = await server.ssrLoadModule('/src/components/overview/HostReadiness.svelte');

const source = name => readFile(new URL(`./src/${name}`, import.meta.url), 'utf8');

test('Help imports no Updates, Power, Password or Sessions panel, nor the integrations', async () => {
  const help = await source('pages/Maintenance.svelte');
  for (const panel of ['UpdatesPanel', 'PowerPanel', 'PasswordPanel', 'SessionsPanel', 'VulkanPanel', 'LimiterPanel']) {
    assert.doesNotMatch(help, new RegExp(`import ${panel}\\b`), panel);
  }
});

test('Help is titled for someone with a problem and keeps the order of the page map', async () => {
  const help = await source('pages/Maintenance.svelte');
  assert.match(help, /<PageHeader title="Help" subtitle="Something is wrong\? Start here\.">/);
  // The host anchor other pages link to stays.
  assert.match(help, /<div id="host"><\/div>/);
  const order = ['<ChildSessionBanner', '<HostPanel', '<DisplaysPanel', '<WindowsSessionPanel', '<CoopHelpPanel', '<LogAnalysisPanel', '<CrashPanel'];
  const at = order.map((tag) => help.indexOf(tag));
  assert.ok(at.every((index) => index > 0), `${at}`);
  assert.deepEqual([...at].sort((a, b) => a - b), at);
});

test('the Vulkan HDR layer and RTSS live in a collapsed Integrations group under Display', async () => {
  const page = await source('pages/Settings.svelte');
  const fold = page.match(/\{#if active\.id === 'display'\}([\s\S]*?)\{\/if\}/)?.[1] ?? '';
  assert.match(fold, /<details class="fold integrations">/);
  assert.doesNotMatch(fold, /<details[^>]*\bopen\b/);
  assert.match(fold, /Integrations/);
  assert.match(fold, /<VulkanPanel \/>/);
  assert.match(fold, /<LimiterPanel \/>/);
});

const metadata = {
  encoder_status: { state: 'ready', h264: true, hevc: true, av1: true, pyrowave: false },
  warnings: [],
  virtual_display: { capable: true, ready: true, reason: '' },
  capture_status: { virtual_display_configured: true, displays: [], error: null },
  audio_enabled: true,
  audio_sinks: [{ id: 'speakers', name: 'Speakers', description: '', adapter: '', default: true, virtual_sink: false }],
};
const readiness = (changes) =>
  render(HostReadiness, { props: { metadata: { ...metadata, ...changes }, error: null, checking: false, onrecheck: () => {} } }).body;

test('a failed readiness check makes Home show Needs attention with a link to Help', () => {
  const failed = readiness({
    encoder_status: { ...metadata.encoder_status, state: 'failed', h264: false, hevc: false, av1: false },
    warnings: [{ code: 'video_encoder', message: 'No video encoder available.' }],
  });
  assert.match(failed, /Needs attention/);
  assert.match(failed, /href="\/maintenance"/);
  const ready = readiness({});
  assert.doesNotMatch(ready, /Needs attention|href="\/maintenance"/);
  assert.match(ready, /Host readiness/);
});

test('the log Copy label is one flex item, so no gap doubles the space before "visible lines"', async () => {
  const logs = await source('pages/Logs.svelte');
  assert.match(logs, /onclick=\{copy\}><span>Copy<span class="extra">\{" "\}visible lines<\/span><\/span><\/Button>/);
  const view = await source('components/logs/LogView.svelte');
  assert.match(view, /\.placeholder \{[^}]*place-items: center;[^}]*min-height: 100%;/);
});

test('the Releases More keeps a 44 px target without pushing its neighbours apart', async () => {
  const updates = await source('components/maintenance/UpdatesPanel.svelte');
  assert.match(updates, /\.more > summary \{[^}]*min-height: var\(--control-height\);\s*margin-block: calc\(\(20px - var\(--control-height\)\) \/ 2\);/);
});
