import assert from 'node:assert/strict';
import { test } from 'node:test';
import { createTestServer } from './test-server.mjs';

globalThis.location = new URL('http://localhost/');
globalThis.window = new EventTarget();
const server = await createTestServer();
const folders = await server.ssrLoadModule('/src/lib/shared-folders.ts');
const { schema } = await server.ssrLoadModule('/src/lib/settings-schema.ts');
const values = await server.ssrLoadModule('/src/components/settings/values.ts');
const { default: SettingControl } = await server.ssrLoadModule('/src/components/settings/SettingControl.svelte');
const { render } = await server.ssrLoadModule('svelte/server');
const { toDraft, changes, savingEndsStream } = await server.ssrLoadModule('/src/components/devices/device.ts');
const { readFolders, folderErrors, hasFolderErrors, storedFolders, newFolder, validFolderRoot, validFolderId, grantRows } = folders;

const setting = schema.settings.find((entry) => entry.key === 'shared_folders');
const games = { id: 'games', name: 'Games', path: 'D:\\Games' };
const client = (extra) => ({ uuid: 'u', name: 'Phone', perm: 0x071f1f00, enabled: true, connected: true, ...extra });

test('the setting is a folder list editor, not a JSON field', () => {
  assert.equal(setting.control.kind, 'folders');
  assert.equal(setting.default, '[]');
  assert.deepEqual(values.describeDefault(setting), { text: 'None', literal: false });
});

test('folder paths follow the host: a drive of this PC, never a share, a relative path or . and ..', () => {
  for (const path of ['D:\\Games', 'D:/Games', 'C:\\', 'e:\\My Files\\Saves']) assert.ok(validFolderRoot(path), path);
  for (const path of ['', 'Games', 'C:', 'C:Games', '\\\\server\\share', '\\\\?\\C:\\Games', '\\\\.\\C:', 'C:\\a\\..\\b', 'C:\\.', 'C:\\a:stream', '1:\\x']) {
    assert.ok(!validFolderRoot(path), path);
  }
  assert.ok(validFolderId('games-k2fq_2'));
  for (const id of ['', '../x', 'a b', 'ä', 'x'.repeat(65)]) assert.ok(!validFolderId(id), id);
});

test('each folder shows what is wrong with it, and a row left blank is not an error', () => {
  const rows = [
    { id: '', name: '', path: '' },
    { id: '', name: ' ', path: 'D:\\Music' },
    { id: '', name: 'Games', path: 'Games' },
    { id: 'games', name: 'games', path: 'D:\\Games' },
    { id: 'bad id', name: 'Saves', path: '\\\\nas\\saves' },
    { id: 'games', name: 'Other', path: 'D:\\Other' },
  ];
  const errors = folderErrors(rows);
  assert.deepEqual(errors[0], { name: '', path: '', id: '' });
  assert.equal(errors[1].name, 'Enter a name.');
  assert.equal(errors[1].path, '');
  assert.equal(errors[2].name, 'Another folder has this name.');
  assert.match(errors[2].path, /drive of this PC/);
  assert.equal(errors[3].name, 'Another folder has this name.');
  assert.match(errors[3].id, /Another folder has the saved id “games”/);
  assert.match(errors[4].path, /Network shares/);
  assert.match(errors[4].id, /refuses the saved id “bad id”/);
  assert.match(errors[5].id, /saved id “games”/);
  assert.ok(hasFolderErrors(rows));
  assert.ok(!hasFolderErrors([games, { id: '', name: '', path: '' }]));
  assert.match(folderErrors([{ id: '', name: 'Games', path: ' ' }])[0].path, /full path/);
});

test('a new folder gets an id from its name and random letters; a saved one keeps its id when renamed', () => {
  const added = newFolder();
  assert.match(added.fresh, /^[a-z2-7]{4}$/);
  const [renamed, fresh, blankName] = storedFolders([
    { ...games, name: ' Spiele ' },
    { id: '', name: 'Spiele & Speicherstände', path: ' D:\\Saves ', fresh: 'k2fq' },
    { id: '', name: '游戏', path: 'D:\\Other', fresh: 'ab3d' },
    { id: '', name: '', path: '', fresh: 'zzzz' },
  ]);
  assert.deepEqual(renamed, { id: 'games', name: 'Spiele', path: 'D:\\Games' });
  assert.deepEqual(fresh, { id: 'spiele-speicherstande-k2fq', name: 'Spiele & Speicherstände', path: 'D:\\Saves' });
  assert.equal(blankName.id, 'folder-ab3d');
  assert.equal(storedFolders([{ id: '', name: 'x', path: '', fresh: '' }, { id: '', name: '', path: '' }]).length, 1);
  // Two folders added with the same letters still get two ids.
  const twins = storedFolders([{ id: 'a-k2fq', name: 'a', path: 'D:\\a' }, { id: '', name: 'a', path: 'D:\\b', fresh: 'k2fq' }]);
  assert.deepEqual(twins.map((folder) => folder.id), ['a-k2fq', 'a-k2fq-2']);
  for (const folder of [renamed, fresh, blankName]) assert.ok(validFolderId(folder.id), folder.id);
  assert.ok(validFolderId(storedFolders([{ id: '', name: 'n'.repeat(200), path: 'D:\\n', fresh: 'abcd' }])[0].id));
});

test('two folders added with the same name never share an id, so neither inherits the other’s grants', () => {
  const ids = new Set();
  for (let n = 0; n < 50; n++) ids.add(storedFolders([{ ...newFolder(), name: 'Games', path: 'D:\\Games' }])[0].id);
  assert.ok(ids.size > 45, `${ids.size} ids`);
  assert.ok(!ids.has('games'));
});

test('the setting is saved as the host reads it, and emptied back to the default', () => {
  const edited = [games, { id: '', name: 'Music', path: 'D:\\Music', fresh: 'mmmm' }, { id: '', name: '', path: '', fresh: 'x' }];
  assert.deepEqual(values.patchValue(setting, edited), [games, { id: 'music-mmmm', name: 'Music', path: 'D:\\Music' }]);
  assert.equal(values.patchValue(setting, []), null);
  assert.equal(values.patchValue(setting, [{ id: '', name: '', path: '' }]), null);
  // GET /api/config gives the stored text parsed, or the text when it isn't JSON.
  assert.deepEqual(values.normalise(setting, JSON.stringify([games])), [games]);
  assert.deepEqual(values.normalise(setting, [games]), [games]);
  assert.deepEqual(
    values.computeChanges(new Map([['shared_folders', setting]]), { shared_folders: [{ ...games }] }, { shared_folders: [games] }),
    {},
  );
});

test('a saved value that is not a folder list is reported and replaced only by an edit', () => {
  assert.match(readFolders('[{"id": "games",').problem, /^it is not JSON/);
  assert.equal(readFolders({ id: 'games' }).problem, 'it is not a list');
  assert.equal(readFolders(['D:\\Games']).problem, 'an entry is not a folder');
  assert.deepEqual(readFolders(''), { folders: [], problem: '' });
  const html = render(SettingControl, {
    props: { setting, id: 'f', value: values.normalise(setting, '[{"id": "games",'), onchange: () => {} },
  }).body;
  assert.match(html, /The saved list can’t be read, so no folder is shared: it is not JSON/);
});

test('the editor shows each folder with its errors beside the field', () => {
  const value = [games, { id: '', name: '', path: 'Games', fresh: 'abcd' }];
  const html = render(SettingControl, { props: { setting, id: 'f', value, onchange: () => {} } }).body;
  assert.match(html, /value="Games"/);
  assert.match(html, /value="D:\\Games"/);
  assert.match(html, /Id: games/);
  assert.match(html, /Gets an id from its name when saved/);
  assert.match(html, /id="f-1-name-error"[^>]*>Enter a name\./);
  assert.match(html, /id="f-1-path-error"[^>]*>Enter a folder on a drive of this PC/);
  assert.match(html, /aria-describedby="f-1-path-error"/);
  assert.match(html, /Add folder/);
  const empty = render(SettingControl, { props: { setting, id: 'f', value: [], onchange: () => {} } }).body;
  assert.match(empty, /No folders are shared\./);
});

test('a device’s grants are read as the host reads them and sent sorted, or null for none', () => {
  const draft = toDraft(client({ shared_folders: { music: 'write', games: 'read', junk: 'all', bad: true } }));
  assert.deepEqual(draft.shared_folders, { music: 'write', games: 'read' });
  assert.deepEqual(toDraft(client({})).shared_folders, {});
  assert.deepEqual(toDraft(client({ shared_folders: ['games'] })).shared_folders, {});
  const none = toDraft(client({}));
  assert.deepEqual(changes(none, { ...none, shared_folders: { music: 'read', games: 'write' } }), {
    shared_folders: { games: 'write', music: 'read' },
  });
  assert.deepEqual(changes(draft, { ...draft, shared_folders: {} }), { shared_folders: null });
  // The same grants in another order are no change.
  assert.deepEqual(changes(draft, { ...draft, shared_folders: { games: 'read', music: 'write' } }), {});
});

test('taking a folder or its writing away ends the device’s stream; adding or widening a grant doesn’t', () => {
  const draft = toDraft(client({ shared_folders: { games: 'write', music: 'read' } }));
  const ends = (grants, connected = true) => savingEndsStream(connected, draft, { ...draft, shared_folders: grants });
  assert.ok(ends({ music: 'read' }));
  assert.ok(ends({ games: 'read', music: 'read' }));
  assert.ok(ends({}));
  assert.ok(!ends({ games: 'write', music: 'write', saves: 'read' }));
  assert.ok(!ends({}, false));
});

test('the Devices editor lists every shared folder, then grants for folders no longer shared', () => {
  const rows = grantRows(
    [games, { id: 'music', name: 'Music', path: 'D:\\Music' }, { id: 'bad id', name: 'Bad', path: 'D:\\Bad' }],
    { music: 'write', old: 'read' },
  );
  assert.deepEqual(rows, [
    { id: 'games', name: 'Games', path: 'D:\\Games', access: '', gone: false },
    { id: 'music', name: 'Music', path: 'D:\\Music', access: 'write', gone: false },
    { id: 'old', name: 'old', path: '', access: 'read', gone: true },
  ]);
  assert.deepEqual(folders.withGrant({ games: 'read' }, 'games', ''), {});
  assert.deepEqual(folders.withGrant({ games: 'read' }, 'music', 'write'), { games: 'read', music: 'write' });
});
