import assert from 'node:assert/strict';
import { readFileSync } from 'node:fs';
import { after, before, test } from 'node:test';
import { createTestServer } from './test-server.mjs';

// Rubylight 7.0's command palette (Ctrl+K): what quick-switch-70.ts lists and QuickSwitcher draws.
globalThis.location = new URL('http://localhost/');
globalThis.window = new EventTarget();
let server, render, lib, QuickSwitcher;
before(async () => {
  server = await createTestServer();
  ({ render } = await server.ssrLoadModule('svelte/server'));
  lib = await server.ssrLoadModule('/src/lib/quick-switch-70.ts');
  ({ default: QuickSwitcher } = await server.ssrLoadModule('/src/components/QuickSwitcher.svelte'));
});
after(() => server?.close());

const source = name => readFileSync(new URL(`./src/${name}`, import.meta.url), 'utf8');
const apps = [
  { uuid: 'steam-620', name: 'Portal 2', 'steam-id': '620', 'synced-by': 'steam', 'image-path': 'covers/portal.png' },
  { uuid: 'witcher', name: 'The Witcher 3', 'launcher-source': 'gog' },
];
const clients = [
  { uuid: 'phone', name: 'Pixel 9', enabled: true, connected: true, perm: 0 },
  { uuid: 'tv', name: 'Living room TV', enabled: true, connected: false, perm: 0 },
  { uuid: 'old', name: 'Old tablet', enabled: false, connected: false, perm: 0 },
];
const found = { apps, clients };
const idle = { theme: 'system', streams: 0 };
const labels = (groups, id) => groups.find(group => group.id === id)?.entries.map(entry => entry.label) ?? [];

test('with nothing typed: the everyday pages, the host actions and diagnostics', () => {
  const groups = lib.palette('', found, idle);
  assert.deepEqual(groups.map(group => group.title), ['Pages', 'Host actions', 'Diagnostics']);
  assert.deepEqual(labels(groups, 'pages'), ['Home', 'Games', 'Devices', 'Settings', 'Help']);
  assert.deepEqual(labels(groups, 'actions'),
    ['Pair a device', 'Sync games now', 'Restore displays', 'Use the light theme', 'Use the dark theme', 'Restart Rubylight']);
  assert.deepEqual(labels(groups, 'diagnostics'), ['Troubleshoot', 'Sessions', 'Logs', 'Download support bundle']);
});

test('host actions follow the host: only other themes, and Disconnect every stream only while a device streams', () => {
  const themes = context => lib.hostActions(context).filter(entry => entry.action?.startsWith('theme-')).map(entry => entry.action);
  assert.deepEqual(themes({ theme: 'system', streams: 0 }), ['theme-light', 'theme-dark']);
  assert.deepEqual(themes({ theme: 'dark', streams: 0 }), ['theme-light', 'theme-system']);
  assert.deepEqual(themes({ theme: 'light', streams: 0 }), ['theme-dark', 'theme-system']);
  assert.equal(lib.hostActions(idle).some(entry => entry.action === 'stop'), false);
  const stop = lib.hostActions({ theme: 'system', streams: 2 }).find(entry => entry.action === 'stop');
  assert.equal(stop.label, 'Disconnect every stream');
  assert.equal(stop.detail, '2 streams');
  assert.equal(lib.hostActions({ theme: 'system', streams: 1 }).find(entry => entry.action === 'stop').detail, '1 stream');
  // Restoring displays writes the display layout: the palette only leads to the panel that asks first.
  const displays = lib.hostActions(idle).find(entry => entry.label === 'Restore displays');
  assert.equal(displays.href, '/troubleshoot#displays');
  assert.equal(displays.action, undefined);
  assert.match(source('components/maintenance/DisplaysPanel.svelte'), /<Panel id="displays" title="Displays"/);
});

test('a query finds across the groups: the closest group first, settings last, diagnostics apart from pages', () => {
  const theme = lib.palette('dark theme', found, idle);
  assert.equal(theme[0].id, 'actions');
  assert.equal(theme[0].entries[0].action, 'theme-dark');
  const logs = lib.palette('logs', found, idle);
  assert.equal(logs[0].id, 'diagnostics');
  assert.deepEqual(labels(logs, 'pages').includes('Logs'), false, 'a diagnostics page is listed once');
  assert.equal(labels(lib.palette('portal', found, idle), 'games')[0], 'Portal 2');
  const bitrate = lib.palette('bitrate', found, idle);
  assert.equal(bitrate.at(-1).id, 'settings');
  for (const group of bitrate) assert.ok(group.entries.length <= lib.PALETTE_LIMITS[group.id], group.id);
  assert.equal(labels(lib.palette('stop', found, { theme: 'system', streams: 1 }), 'actions')[0], 'Disconnect every stream');
  assert.deepEqual(lib.palette('zzzz no such thing', found, idle), []);
});

test('games carry their cover, devices their state, and Enter says what it does', () => {
  const game = lib.palette('portal', found, idle).find(group => group.id === 'games').entries[0];
  assert.deepEqual(game.cover, { uuid: 'steam-620', path: 'covers/portal.png' });
  const devices = lib.palette('pixel living old', found, idle);
  assert.deepEqual(devices, [], 'every word must match');
  const state = name => lib.palette(name, found, idle).find(group => group.id === 'devices').entries[0].status;
  assert.equal(state('pixel'), 'connected');
  assert.equal(state('living'), 'idle');
  assert.equal(state('old tablet'), 'disabled');
  assert.equal(lib.enterVerb({ id: 'a', label: 'a', icon: 'power', action: 'restart' }), 'Run');
  assert.equal(lib.enterVerb({ id: 'p', label: 'p', icon: 'help', href: '/help' }), 'Open');
  assert.equal(lib.enterVerb(undefined), 'Open');
});

test('the palette draws its groups, the legend and nothing a mod is named by', () => {
  const out = render(QuickSwitcher, { props: { open: true, theme: 'dark' } }).body;
  for (const title of ['Pages', 'Host actions', 'Diagnostics']) assert.match(out, new RegExp(`class="group-title[^"]*"[^>]*>${title}<`));
  assert.equal((out.match(/role="option"/g) ?? []).length, 15);
  assert.match(out, /Use the light theme/);
  assert.match(out, /Follow the system theme/);
  assert.doesNotMatch(out, /Use the dark theme/);
  assert.match(out, /<kbd[^>]*>↑<\/kbd><kbd[^>]*>↓<\/kbd> Move/);
  assert.match(out, /<kbd[^>]*>Esc<\/kbd> Close/);
  const palette = source('lib/quick-switch-70.ts') + source('components/QuickSwitcher.svelte');
  assert.doesNotMatch(palette, /nucleus|\banr\b/i);
  // The palette's writes are the console's own: a confirmed disconnect, and the theme through App.
  const switcher = source('components/QuickSwitcher.svelte');
  assert.match(switcher, /const ok = await confirm\(\{[\s\S]*?confirm: 'Disconnect',[\s\S]*?\}\);\s*if \(!ok\) return;/);
  assert.match(source('App.svelte'), /<QuickSwitcher bind:open=\{switcherOpen\} \{theme\} ontheme=\{setTheme\}/);
});
