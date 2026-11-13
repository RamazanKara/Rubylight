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

const inOrder = (page, order) => {
  const at = order.map((tag) => page.indexOf(tag));
  assert.ok(at.every((index) => index > 0), `${order}: ${at}`);
  assert.deepEqual([...at].sort((a, b) => a - b), at);
};

test('Help and Troubleshoot import no Updates, Power, Password or Sessions panel, nor the integrations', async () => {
  const pages = (await source('pages/Help.svelte')) + (await source('pages/Troubleshoot.svelte'));
  for (const panel of ['UpdatesPanel', 'PowerPanel', 'PasswordPanel', 'SessionsPanel', 'VulkanPanel', 'LimiterPanel']) {
    assert.doesNotMatch(pages, new RegExp(`import ${panel}\\b`), panel);
  }
});

test('Help is connecting, log findings and the support bundle, and links to Troubleshoot', async () => {
  const help = await source('pages/Help.svelte');
  assert.match(help, /<PageHeader title="Help" subtitle="Connection, log findings and the support bundle\.">/);
  assert.match(help, /<Button href="\/troubleshoot" icon="maintenance">Troubleshoot<\/Button>/);
  inOrder(help, ['<ConnectionHelp', '<LogAnalysisPanel', '<SupportBundlePanel']);
  // Recovery is Troubleshoot's, and co-op's folder has its own home.
  for (const tag of ['<HostPanel', '<DisplaysPanel', '<WindowsSessionPanel', '<CoopHelpPanel', '<CrashPanel', '<NucleusFolder']) {
    assert.doesNotMatch(help, new RegExp(tag), tag);
  }
});

test('Troubleshoot holds the recovery Help had, in the order of the page map, under Advanced', async () => {
  const page = await source('pages/Troubleshoot.svelte');
  assert.match(page, /<PageHeader title="Troubleshoot" subtitle="Restart, displays, co-op, crash reports and settings backup\.">/);
  // The host anchor links may point at stays, on the section so it adds no extra gap.
  assert.match(page, /<Section title="Rubylight" anchor="host" quiet>/);
  assert.doesNotMatch(page, /<div id="host"><\/div>/);
  inOrder(page, ['<ChildSessionBanner', '<HostPanel', '<DisplaysPanel', '<WindowsSessionPanel', '<CoopHelpPanel', '<CrashPanel', '<BackupPanel']);
  assert.match(page, /<Section title="Settings backup" anchor="backup" quiet>/);
  assert.doesNotMatch(page, /<LogAnalysisPanel|moved/);
  const nav = await source('lib/navigation.ts');
  const advanced = nav.slice(nav.indexOf('export const advanced'));
  assert.match(advanced, /path: '\/troubleshoot',\s*label: 'Troubleshoot'/);
  assert.match(nav.slice(0, nav.indexOf('export const advanced')), /path: '\/help', label: 'Help'/);
  const app = await source('App.svelte');
  assert.match(app, /route\.path === '\/help'\) return Help;/);
  assert.match(app, /route\.path === '\/troubleshoot'\) return Troubleshoot;/);
});

test('Help draws this PC’s name and address with a copy button, and pairing one click away', async () => {
  const { default: ConnectionHelp } = await server.ssrLoadModule('/src/components/help/ConnectionHelp.svelte');
  const draw = (changes) =>
    render(ConnectionHelp, { props: { metadata: { host_name: 'Gaming PC', pc_address: '192.168.4.20', pc_addresses: ['192.168.4.20', '10.0.0.7'], paired_devices: 2, ...changes } } }).body;
  const out = draw({});
  assert.match(out, /Gaming PC/);
  assert.match(out, /192\.168\.4\.20/);
  assert.match(out, /Copy address/);
  assert.match(out, /Other networks[\s\S]*10\.0\.0\.7/);
  assert.match(out, /href="\/devices\?pair=qr"/);
  assert.doesNotMatch(out, /class="button primary/);
  // Nothing paired yet: Pair a device is the page's main action.
  const fresh = draw({ paired_devices: 0, pc_addresses: ['192.168.4.20'] });
  assert.match(fresh, /None yet/);
  assert.match(fresh, /class="button primary sm[^"]*"[^>]*href="\/devices\?pair=qr"/);
  assert.doesNotMatch(fresh, /Other networks/);
  assert.match(render(ConnectionHelp, { props: { metadata: null } }).body, /Loading/);
});

test('the Vulkan HDR layer and RTSS live in a collapsed Integrations fold under Picture', async () => {
  const page = await source('pages/Settings.svelte');
  const fold = page.match(/\{#if active\.id === 'picture'\}([\s\S]*?)\{\/if\}/)?.[1] ?? '';
  assert.match(fold, /<Disclosure\s[^>]*title=\{INTEGRATIONS\}/);
  assert.match(fold, /open=\{integrationsOpen\}/);
  assert.match(page, /let integrationsOpen = \$state\(false\);/);
  assert.match(page, /const INTEGRATIONS = 'Integrations';/);
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
  assert.match(failed, /href="\/help"/);
  const ready = readiness({});
  assert.doesNotMatch(ready, /Needs attention|href="\/help"/);
  assert.match(ready, />Checks</);
});

test('the log Copy label is one flex item, so no gap doubles the space before "visible lines"', async () => {
  const logs = await source('pages/Logs.svelte');
  assert.match(logs, /onclick=\{copy\}><span>Copy<span class="extra">\{" "\}visible lines<\/span><\/span><\/Button>/);
  const view = await source('components/logs/LogView.svelte');
  assert.match(view, /\.placeholder \{[^}]*place-items: center;[^}]*min-height: 100%;/);
});

test('the Updates folds keep a 44 px target without pushing their neighbours apart', async () => {
  const updates = await source('components/maintenance/UpdatesPanel.svelte');
  assert.match(updates, /<Disclosure variant="inline" title="Schedule and release notes">/);
  assert.doesNotMatch(updates, /<details/);
  const disclosure = await source('components/Disclosure.svelte');
  assert.match(disclosure, /\.inline > summary \{[^}]*min-height: var\(--control-height\);\s*margin-block: calc\(\(20px - var\(--control-height\)\) \/ 2\);/);
});
