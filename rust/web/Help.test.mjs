import assert from 'node:assert/strict';
import { readFile } from 'node:fs/promises';
import { test } from 'node:test';
import { createTestServer } from './test-server.mjs';

globalThis.location = new URL('http://localhost/');
globalThis.window = new EventTarget();
const server = await createTestServer();
const { pending } = await server.ssrLoadModule('/src/components/maintenance/child-session.ts');
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
  const order = ['<HostPanel', '<DisplaysPanel', '<WindowsSessionPanel', '<LogAnalysisPanel', '<ChildSessionPanel', '<CrashPanel'];
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

const off = {
  mode: 'virtual_display',
  set_up: false,
  viewer: null,
  problem: null,
  missing: ['Remote Desktop'],
  missing_error: null,
  display_error: null,
  unavailable: null,
};

test('Help shows the child session only while there is something to use, restore or fix', () => {
  assert.equal(pending(off), false);
  assert.equal(pending({ ...off, set_up: true }), true, 'restore is pending');
  assert.equal(pending({ ...off, mode: 'child_session' }), true, 'Background mode uses it');
  assert.equal(pending({ ...off, problem: 'Background streaming stopped' }), true);
  assert.equal(pending({ ...off, missing_error: 'Access denied' }), true);
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
