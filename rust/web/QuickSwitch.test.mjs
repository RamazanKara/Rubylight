import assert from 'node:assert/strict';
import { readFile } from 'node:fs/promises';
import { test } from 'node:test';
import { createTestServer } from './test-server.mjs';

globalThis.location = new URL('http://localhost/');
globalThis.window = new EventTarget();
const server = await createTestServer();
const { render } = await server.ssrLoadModule('svelte/server');
const switcher = await server.ssrLoadModule('/src/lib/quick-switch.ts');
const { results, ranked, settingLink, ACTIONS, LIMITS } = switcher;
const { canonical, gamePage, MOVED } = await server.ssrLoadModule('/src/lib/router.svelte.ts');
const { schema } = await server.ssrLoadModule('/src/lib/settings-schema.ts');
const QuickSwitcher = (await server.ssrLoadModule('/src/components/QuickSwitcher.svelte')).default;

const source = (name) => readFile(new URL(`./src/${name}`, import.meta.url), 'utf8');
const apps = [
  { uuid: 'desktop', name: 'Desktop' },
  { uuid: 'steam-620', name: 'Portal 2', 'steam-id': '620', 'synced-by': 'steam', 'image-path': 'covers/portal.png' },
  { uuid: 'witcher', name: 'The Witcher 3', 'launcher-source': 'gog', 'not-installed': true },
  { name: 'Old app without a uuid' },
];
const clients = [
  { uuid: 'phone', name: 'Pixel 9', enabled: true, connected: true, perm: 0 },
  { uuid: 'tv', name: 'Living room TV', enabled: false, connected: false, perm: 0 },
];
const found = { apps, clients };
const titles = (groups) => groups.map((group) => group.title);
const labels = (groups, id) => groups.find((group) => group.id === id)?.entries.map((entry) => entry.label) ?? [];

test('with nothing typed it offers the everyday pages and the actions', () => {
  const groups = results('  ', found);
  assert.deepEqual(titles(groups), ['Pages', 'Actions']);
  assert.deepEqual(labels(groups, 'pages'), ['Home', 'Games', 'Devices', 'Settings', 'Help']);
  assert.deepEqual(labels(groups, 'actions'), ['Pair a device', 'Sync games now', 'Restart Rubylight', 'Download support bundle']);
  assert.equal(ACTIONS.find((entry) => entry.label === 'Pair a device').href, '/devices?pair=qr');
});

test('pages under Advanced are found by name and words, and say where they are', () => {
  const [pages] = results('troub', found);
  assert.equal(pages.entries[0].label, 'Troubleshoot');
  assert.equal(pages.entries[0].href, '/troubleshoot');
  assert.equal(pages.entries[0].detail, 'Advanced');
  // Restart, the displays and crash reports are Troubleshoot's.
  assert.equal(results('displays', found)[0].entries[0].href, '/troubleshoot');
  assert.deepEqual(labels(results('logs', found), 'pages').slice(0, 1), ['Logs']);
});

test('a game opens its page, shows its cover and launcher, and an app without a uuid is left out', () => {
  const groups = results('portal', found);
  assert.deepEqual(titles(groups), ['Games']);
  const [portal] = groups[0].entries;
  assert.equal(portal.href, '/library/steam-620');
  assert.equal(portal.detail, 'Steam');
  assert.deepEqual(portal.cover, { uuid: 'steam-620', path: 'covers/portal.png' });
  assert.equal(results('witcher', found)[0].entries[0].detail, 'Not installed');
  assert.deepEqual(labels(results('old app', found), 'games'), []);
  // Games found by their launcher too.
  assert.deepEqual(labels(results('gog', found), 'games'), ['The Witcher 3']);
});

test('a device goes to Devices and is brought into view there', () => {
  const [devices] = results('pixel', found);
  assert.equal(devices.title, 'Devices');
  assert.deepEqual(devices.entries[0], {
    id: 'device:phone', label: 'Pixel 9', detail: 'Connected', icon: 'devices', href: '/devices', reveal: 'Pixel 9',
  });
  assert.equal(results('living', found)[0].entries[0].detail, 'Disabled');
  // Not loaded yet: no games and no devices, the rest still answers.
  assert.deepEqual(labels(results('pixel', { apps: null, clients: null }), 'devices'), []);
});

test('an action is found by its name before a page that mentions it', () => {
  const groups = results('restart', found);
  assert.equal(groups[0].id, 'actions');
  assert.deepEqual(groups[0].entries.map((entry) => entry.action), ['restart']);
  assert.equal(results('sync', found)[0].entries[0].action, 'sync');
  assert.equal(results('support bundle', found).find((group) => group.id === 'actions').entries[0].action, 'bundle');
});

test('settings are found as All settings finds them and open their row on the tab that holds them', () => {
  const nucleus = schema.settings.find((setting) => setting.key === 'nucleus_path');
  const groups = results(nucleus.label, found);
  const settings = groups.find((group) => group.id === 'settings');
  const entry = settings.entries.find((candidate) => candidate.id === 'setting:nucleus_path');
  assert.ok(entry, 'the Nucleus Co-op folder is found by its name');
  assert.equal(entry.href, settingLink('nucleus_path'));
  assert.match(entry.href, /^\/settings\/[a-z]+\?setting=nucleus_path$/);
  assert.match(entry.detail, /\S/);
  // An unknown key still leads somewhere: All settings' search for it.
  assert.match(settingLink('no_such_key'), /\?q=no_such_key$/);
});

test('each group stays short, and more settings than fit open All settings’ search', () => {
  const groups = results('a', found);
  for (const group of groups) assert.ok(group.entries.length <= LIMITS[group.id], group.id);
  const settings = groups.find((group) => group.id === 'settings');
  const more = settings.entries.at(-1);
  assert.equal(more.id, 'settings:all');
  assert.match(more.label, /^All \d+ settings for “a”$/);
  assert.match(more.href, /^\/settings\/[a-z]+\?q=a$/);
});

test('the name’s start ranks first, then the start of a word, then anywhere, then only the detail', () => {
  const entries = [
    { id: '1', label: 'Overwatch 2', icon: 'library' },
    { id: '2', label: 'Hades', icon: 'library', keywords: 'roguelike over' },
    { id: '3', label: 'Game Over', icon: 'library' },
    { id: '4', label: 'Clover', icon: 'library' },
    { id: '5', label: 'Over', icon: 'library' },
  ];
  assert.deepEqual(ranked(entries, ['over']).map((entry) => entry.label), ['Over', 'Overwatch 2', 'Game Over', 'Clover', 'Hades']);
  assert.deepEqual(ranked(entries, ['nothing']), []);
  // Keywords count from the start of a word: "ike" is inside "roguelike" and finds nothing.
  assert.deepEqual(ranked(entries, ['ike']), []);
});

test('the group with the closest match comes first, settings last, so Enter opens it', () => {
  const groups = results('po', found);
  assert.equal(groups[0].id, 'games');
  assert.equal(groups[0].entries[0].label, 'Portal 2');
  assert.equal(groups.at(-1).id, 'settings');
  // Help's keyword "support" holds "po", but not at a word's start.
  assert.ok(!labels(groups, 'pages').includes('Help'), labels(groups, 'pages'));
  // On a tie the fixed order holds: the Devices page before a device whose name starts the same.
  assert.deepEqual(titles(results('dev', { apps: [], clients: [{ uuid: 'd', name: 'Devices hub', enabled: true, connected: false, perm: 0 }] })).slice(0, 1), ['Pages']);
});

test('the old Help address opens Help, and only a game’s own page morphs its cover', () => {
  assert.deepEqual(MOVED, { '/maintenance': '/help' });
  assert.equal(canonical('/maintenance'), '/help');
  assert.equal(canonical('/maintenance#host'), '/help#host');
  assert.equal(canonical('/maintenance?tab=1'), '/help?tab=1');
  assert.equal(canonical('/devices?pair=qr'), '/devices?pair=qr');
  assert.equal(canonical('/maintenance/other'), '/maintenance/other');
  assert.equal(gamePage('/library/steam-620'), 'steam-620');
  assert.equal(gamePage('/library/a%20b'), 'a b');
  for (const path of ['/library', '/library/new', '/library/import', '/devices', '/']) assert.equal(gamePage(path), null, path);
});

test('the switcher is a combobox over a listbox, its first result marked, on the console’s Dialog', () => {
  const out = render(QuickSwitcher, { props: { open: true } }).body;
  assert.match(out, /<dialog/);
  assert.match(out, /role="combobox"/);
  assert.match(out, /aria-label="Search games, devices, settings and actions"/);
  assert.match(out, /aria-autocomplete="list"/);
  assert.match(out, /role="listbox"/);
  assert.equal((out.match(/role="option"/g) ?? []).length, 9);
  assert.equal((out.match(/aria-selected="true"/g) ?? []).length, 1);
  assert.match(out, /role="status"/);
});

test('the shell mounts the switcher behind Ctrl+K or ⌘K and a Search button, and routes Help and Troubleshoot', async () => {
  const app = await source('App.svelte');
  assert.match(app, /<QuickSwitcher bind:open=\{switcherOpen\}/);
  assert.match(app, /aria-keyshortcuts="Control\+K Meta\+K"/);
  assert.match(app, /event\.code !== 'KeyK' && event\.key\.toLowerCase\(\) !== 'k'/);
  // A confirmation that is open keeps the keyboard.
  assert.match(app, /if \(!switcherOpen && document\.querySelector\('dialog\[open\]'\)\) return;/);
  // Page changes fade, the shell stays put, a cover morphs.
  assert.match(app, /view-transition-name: page;/);
  assert.match(app, /::view-transition-group\(game-cover\)/);
  const cover = await source('components/library/AppCover.svelte');
  assert.equal((cover.match(/data-cover=\{uuid\}/g) ?? []).length, 2, 'every cover, with art or without, can morph');
  const router = await source('lib/router.svelte.ts');
  assert.match(router, /prefers-reduced-motion: reduce/);
  assert.match(router, /startViewTransition/);
});
