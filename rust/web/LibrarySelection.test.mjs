import assert from 'node:assert/strict';
import { readFileSync } from 'node:fs';
import { test } from 'node:test';
import { createTestServer } from './test-server.mjs';

const server = await createTestServer();
const selection = await server.ssrLoadModule('/src/components/library/selection.ts');
const { api } = await server.ssrLoadModule('/src/lib/api.ts');
const grid = readFileSync(new URL('./src/components/library/LibraryGrid.svelte', import.meta.url), 'utf8');
const sources = readFileSync(new URL('./src/components/library/LauncherSources.svelte', import.meta.url), 'utf8');

test('each app is put under the launcher its game comes from, and the user’s own programs under none', () => {
  const { launcherOf, launcherCounts } = selection;
  const apps = [
    { name: 'Mirage', uuid: 'm', 'launcher-source': 'ubisoft', 'launcher-id': '13504' },
    { name: 'Portal 2', uuid: 'p', 'steam-id': '620' },
    { name: 'By command', uuid: 'c', cmd: 'cmd /c start steam://rungameid/440' },
    { name: 'Kart', uuid: 'k', 'playnite-id': 'aa' },
    { name: 'Notepad', uuid: 'n', cmd: 'C:/Windows/notepad.exe' },
  ];
  assert.deepEqual(apps.map(launcherOf), ['ubisoft', 'steam', 'steam', 'playnite', null]);
  assert.deepEqual(launcherCounts(apps), [
    { launcher: 'playnite', name: 'Playnite', count: 1 },
    { launcher: 'steam', name: 'Steam', count: 2 },
    { launcher: 'ubisoft', name: 'Ubisoft Connect', count: 1 },
  ]);
});

test('the editor’s Delete offers Restore for a launcher’s game and stays final for the user’s own app', () => {
  const { deleteMessage } = selection;
  for (const app of [
    { name: 'Mirage', uuid: 'm', 'launcher-source': 'ubisoft', 'launcher-id': '13504' },
    { name: 'Portal 2', uuid: 'steam-620' },
    { name: 'Dota 2', uuid: '53544541-4D00-5000-8000-00000000023A' },
    { name: 'Kart', uuid: 'k', 'playnite-id': 'aa' },
  ]) {
    assert.match(deleteMessage(app), /Restore removed games/, app.name);
    assert.doesNotMatch(deleteMessage(app), /can’t be undone/, app.name);
  }
  assert.match(deleteMessage({ name: 'Notepad', uuid: 'n', cmd: 'C:/Windows/notepad.exe' }), /can’t be undone/);
  const editor = readFileSync(new URL('./src/components/library/AppEditor.svelte', import.meta.url), 'utf8');
  assert.match(editor, /message: deleteMessage\(draft\.saved\)/);
});

test('select all adds the shown apps to the selection, and a click switches one', () => {
  const { selectApps, toggled } = selection;
  let picked = selectApps(new Set(['a']), [{ uuid: 'b' }, { name: 'No uuid yet' }, { uuid: 'a' }]);
  assert.deepEqual([...picked].sort(), ['a', 'b']);
  picked = toggled(picked, 'a');
  assert.deepEqual([...picked], ['b']);
  assert.deepEqual([...toggled(picked, 'c')].sort(), ['b', 'c']);
});

test('removed games are named from the launchers, and ones no longer installed by their launcher', () => {
  const games = [{ source: 'ubisoft', id: '13504', name: 'Mirage' }, { source: 'steam', id: '620', name: 'Portal 2' }];
  assert.deepEqual(selection.removedGames(['steam:620', 'ubisoft:13504', 'epic:gone', 'playnite:aa'], games), [
    { key: 'epic:gone', launcher: 'Epic Games', name: 'A game no longer installed (gone)' },
    { key: 'playnite:aa', launcher: 'Playnite', name: 'A Playnite game' },
    { key: 'steam:620', launcher: 'Steam', name: 'Portal 2' },
    { key: 'ubisoft:13504', launcher: 'Ubisoft Connect', name: 'Mirage' },
  ]);
});

test('a bulk remove and a restore are one request each, not one per game', async () => {
  const seen = [];
  globalThis.fetch = async (path, init) => {
    if (path === '/api/csrf-token') return new Response('{"csrf_token":"t"}', { status: 200 });
    seen.push({ path, method: init.method, body: JSON.parse(init.body) });
    return new Response('{"status":true,"deleted":2,"restored":1}', { status: 200 });
  };
  try {
    await api.apps.removeMany(['a', 'b']);
    await api.library.restore(['steam:620']);
  } finally {
    delete globalThis.fetch;
  }
  assert.deepEqual(seen, [
    { path: '/api/apps/delete', method: 'POST', body: { uuids: ['a', 'b'] } },
    { path: '/api/library/restore', method: 'POST', body: { games: ['steam:620'] } },
  ]);
});

test('select mode picks apps instead of opening, launching or dragging them', () => {
  assert.match(grid, /!selecting && !reordering/, 'no drag while selecting');
  assert.match(grid, /\{#if selecting && app\.uuid\}[\s\S]*?type="checkbox"/);
  assert.match(grid, /\{#if selecting\}\s*<!--/, 'the card actions step aside');
  assert.match(grid, /api\.apps\.removeMany\(/);
});

test('the restore offer shows only with removed games and syncs after the restore', () => {
  assert.match(sources, /\{#if removed\.length > 0\}[\s\S]*?Restore removed games/);
  assert.match(sources, /api\.library\.restore\(restoreKeys\);[\s\S]*?await syncAll\(false\)/);
  assert.doesNotMatch(sources + grid, /Experimental/);
});
