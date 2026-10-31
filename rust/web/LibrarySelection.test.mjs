import assert from 'node:assert/strict';
import { readFileSync } from 'node:fs';
import { test } from 'node:test';
import { createTestServer } from './test-server.mjs';

const server = await createTestServer();
const selection = await server.ssrLoadModule('/src/components/library/selection.svelte.ts');
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
    return new Response('{"status":true,"deleted":2,"kept":[],"restored":1}', { status: 200 });
  };
  try {
    const removal = await api.apps.removeMany(['a', 'b']);
    assert.equal(removal.deleted, 2);
    await api.library.restore(['steam:620']);
  } finally {
    delete globalThis.fetch;
  }
  assert.deepEqual(seen, [
    { path: '/api/apps/delete', method: 'POST', body: { uuids: ['a', 'b'] } },
    { path: '/api/library/restore', method: 'POST', body: { games: ['steam:620'] } },
  ]);
});

const library = [
  { name: 'Portal 2', uuid: 'portal', 'steam-id': '620' },
  { name: 'Mirage', uuid: 'mirage', 'launcher-source': 'ubisoft', 'launcher-id': '13504' },
  { name: 'Half-Life: Alyx', uuid: 'alyx', 'steam-id': '546560' },
  { name: 'Notepad', uuid: 'notepad', cmd: 'notepad.exe' },
];

test('a selection drops apps that left the list, and a removal sends only those still there', async () => {
  const picks = new selection.AppSelection();
  picks.start();
  picks.addLauncher(library, 'steam');
  picks.toggle('notepad');
  assert.deepEqual(picks.chosen(library).map((app) => app.uuid), ['portal', 'alyx', 'notepad']);
  // A sync dropped Alyx meanwhile: it is neither counted nor sent.
  const now = library.filter((app) => app.uuid !== 'alyx');
  assert.deepEqual(picks.chosen(now).map((app) => app.uuid), ['portal', 'notepad']);
  let sent;
  const message = await picks.remove(now, async (uuids) => {
    sent = uuids;
    return { deleted: 2, kept: [] };
  });
  assert.deepEqual(sent, ['portal', 'notepad']);
  assert.deepEqual(message, { text: 'Removed 2 apps.', tone: 'ok' });
  assert.equal(picks.selecting, false, 'select mode ends after a removal');
  assert.equal(picks.selected.size, 0);
});

test('a failed removal keeps select mode and the selection for another try', async () => {
  const picks = new selection.AppSelection();
  picks.start();
  picks.add(library);
  picks.clear();
  assert.equal(picks.selected.size, 0, 'Select none');
  picks.toggle('mirage');
  await assert.rejects(picks.remove(library, async () => {
    throw new Error('timed out');
  }));
  assert.equal(picks.selecting, true);
  assert.deepEqual([...picks.selected], ['mirage']);
});

test('a partial removal names the apps kept and counts what went', () => {
  const { removalMessage } = selection;
  assert.deepEqual(removalMessage(1, 'Mirage', { deleted: 1 }), { text: 'Removed Mirage.', tone: 'ok' });
  assert.deepEqual(removalMessage(3, '3 apps', { deleted: 2, kept: [] }), {
    text: 'Removed 2 of 3 apps; the others were already gone.',
    tone: 'ok',
  });
  assert.deepEqual(
    removalMessage(3, '3 apps', { deleted: 2, kept: [{ name: 'Mirage', error: 'Neural Rendering could not be removed' }] }),
    { text: 'Removed 2 of 3 apps. Kept Mirage: Neural Rendering could not be removed', tone: 'warn' },
  );
});

test('a restore says what happened, also when the sync after it failed', () => {
  const { restoreMessage } = selection;
  assert.deepEqual(restoreMessage(2, true, true), { text: 'Restored 2 games: the installed ones are back in your library.', tone: 'ok' });
  assert.deepEqual(restoreMessage(1, false, true), { text: 'Restored 1 game: they join your library with the next sync.', tone: 'warn' });
  assert.deepEqual(restoreMessage(2, true, false), { text: 'Restored 2 games: pick them again in Add games.', tone: 'ok' });
});

test('the grid and the launcher panel use the selection and restore logic', () => {
  assert.match(grid, /!selecting && !reordering/, 'no drag while selecting');
  assert.match(grid, /selection\.remove\(apps, api\.apps\.removeMany\)/);
  assert.match(grid, /finally \{[\s\S]*?onremoved\?\.\(\);/, 'the list reloads after a failed removal too');
  assert.match(sources, /\{#if removed\.length > 0\}[\s\S]*?Restore removed games/);
  assert.match(sources, /const synced = await syncAll\(false\);\s*[\s\S]*?if \(!synced\) onsynced\?\.\(\);/);
  assert.doesNotMatch(sources + grid, /Experimental/);
});
